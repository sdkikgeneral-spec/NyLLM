// OllamaAgent — ローカル Ollama デーモンへのHTTP推論バックエンド(設計 §6, §7。
// docs/superpowers/specs/2026-07-18-selectable-inference-backend-design.md)。
//
// 構造(テスト前提の分離):
//   - リクエスト組み立て(build_chat_request)/ レスポンス抽出(extract_answer)/
//     自己申告のパース(parse_self_declaration)/ 申告フォールバック確定
//     (declare_or_fallback)は純関数で、feature なしでもコンパイル・テストされる。
//   - HTTP I/O(OllamaAgent 本体)のみ feature "ollama" 配下(ureq 依存)。
//     既定ビルドは依存を増やさない(CLAUDE.md「デフォルトで軽量ビルド可能」)。
//
// API(設計 §6): POST {endpoint}/api/chat、stream:false、回答は .message.content。

use crate::agent::{heuristic_self_declare, AgentError, SelfDeclaration};
use serde_json::Value;

#[cfg(feature = "ollama")]
use crate::agent::{Agent, AgentConfig};

// ------------------------------------------------------------------
// 純関数(feature なしでもテスト可能。設計 §6「I/Oと分離」)
// ------------------------------------------------------------------

// /api/chat リクエストボディを組み立てる(設計 §6)。
pub fn build_chat_request(model: &str, content: &str) -> Value
{
    serde_json::json!({
        "model": model,
        "messages": [ { "role": "user", "content": content } ],
        "stream": false,
    })
}

// /api/chat レスポンスから回答本文(.message.content)を取り出す(設計 §6)。
pub fn extract_answer(body: &str) -> Result<String, AgentError>
{
    let v: Value = serde_json::from_str(body)
        .map_err(|e| AgentError::Parse(format!("レスポンスがJSONでない: {e}")))?;
    v.get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| AgentError::Parse("レスポンスに .message.content がない".to_string()))
}

// 自己申告(L2)を求める2回目のプロンプト(設計 §7)。
// 質問と生成済み回答を与え、申告JSONのみを返すよう指示する。
pub fn build_declare_prompt(question: &str, answer: &str) -> String
{
    format!(
        "あなたは直前に次の質問へ回答しました。その回答の性質を自己申告してください。\n\
         \n\
         質問: {question}\n\
         回答: {answer}\n\
         \n\
         次のJSONだけを出力してください(説明文・コードフェンス不要):\n\
         {{\"context_independent\": <bool: 前提会話なしで単独回答できるならtrue>, \
         \"factual\": <bool: 客観的事実の記述ならtrue(主観・意見・創作はfalse)>, \
         \"volatility\": <\"permanent\"|\"slow\"|\"volatile\": 内容が時間で変わる速さ>}}"
    )
}

// 申告レスポンスのパース+値検証(純関数。設計 §7 手順2)。
// モデルがコードフェンスや説明文で包むケースに備え、最初の '{' から
// 最後の '}' までを切り出してからパースする。
// 失敗(JSONでない / 必須キー欠落 / bool でない / 不正な volatility 値)は None。
pub fn parse_self_declaration(raw: &str) -> Option<SelfDeclaration>
{
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end < start
    {
        return None;
    }
    let v: Value = serde_json::from_str(&raw[start..=end]).ok()?;
    let context_independent = v.get("context_independent")?.as_bool()?;
    let factual = v.get("factual")?.as_bool()?;
    let volatility = v.get("volatility")?.as_str()?;
    if !matches!(volatility, "permanent" | "slow" | "volatile")
    {
        return None;
    }
    Some(SelfDeclaration
    {
        context_independent,
        factual,
        volatility: volatility.to_string(),
    })
}

// 申告の確定(設計 §7 手順2〜3): パース成功かつ値妥当なら採用、
// 失敗(HTTPエラー / パース不能 / 不正値)は L0ヒューリスティックへフォールバック。
// generated_answer=false: 実LLM経路では生成文かどうか判別できないため、
// 生成文の検知は案4(トリプル分解。pipeline)に委ねる(agent.rs のヘルパー註釈参照)。
//
// 不変条件(設計 §7 手順4): ここで何を採用しても申告は「一票」にすぎず、
// 受け取り側(pipeline::judge_entry / volatility::finalize_volatility)が
// Yes/permanent 側の申告で判定を覆さないこと(§10.1 ルール4)は受け取り側が保証する。
pub fn declare_or_fallback(raw: Result<String, AgentError>, question: &str) -> SelfDeclaration
{
    raw.ok()
        .and_then(|s| parse_self_declaration(&s))
        .unwrap_or_else(|| heuristic_self_declare(question, false))
}

// ------------------------------------------------------------------
// HTTP I/O(feature = "ollama"。ureq = 同期・軽量。設計 §6)
// ------------------------------------------------------------------

#[cfg(feature = "ollama")]
pub struct OllamaAgent
{
    model: String,
    endpoint: String, // 末尾スラッシュなしに正規化済み
    // provenance 用の Agent 名。Architecture §11 R4(説明可能性): エントリの
    // 生成元をモデル単位まで追跡できるよう "ollama:<モデル名>" 形式で保持する
    // (sync.rs が name() を cache.register() の agent 名として記録するため)。
    name: String,
    http: ureq::Agent,
}

#[cfg(feature = "ollama")]
impl OllamaAgent
{
    pub fn new(config: &AgentConfig) -> Self
    {
        let http = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build();
        Self
        {
            model: config.model.clone(),
            endpoint: config.endpoint.trim_end_matches('/').to_string(),
            // Architecture §11 R4: provenance からどのモデルが生成したかを
            // 追跡可能にするため、構築時にモデル名込みの名前を組み立てる。
            name: format!("ollama:{}", config.model),
            http,
        }
    }

    // 1リクエスト1レスポンスのチャット呼び出し(stream:false。設計 §6)。
    // ask(質問)と self_declare(申告プロンプト)が共有する。
    fn chat(&self, content: &str) -> Result<String, AgentError>
    {
        let url = format!("{}/api/chat", self.endpoint);
        let req = build_chat_request(&self.model, content);
        match self.http.post(&url).send_json(req)
        {
            Ok(resp) =>
            {
                let body = resp
                    .into_string()
                    .map_err(|e| AgentError::Parse(format!("レスポンス本文の読取失敗: {e}")))?;
                extract_answer(&body)
            }
            // 非2xx → Http(例: モデル未pull は 404。設計 §6)
            Err(ureq::Error::Status(status, resp)) =>
            {
                let body = resp.into_string().unwrap_or_default();
                Err(AgentError::Http { status, body })
            }
            // 到達不能 / タイムアウト(設計 §6)
            Err(ureq::Error::Transport(t)) => Err(map_transport_error(&t)),
        }
    }
}

// トランスポート層エラーの分類: タイムアウト → Timeout、それ以外 → Unreachable。
// ureq はタイムアウトを io::Error(TimedOut / WouldBlock)として運ぶため
// source を downcast して判定する(メッセージ照合は保険)。
#[cfg(feature = "ollama")]
fn map_transport_error(t: &ureq::Transport) -> AgentError
{
    use std::error::Error as _;
    let msg = t.to_string();
    let timed_out = t
        .source()
        .and_then(|s| s.downcast_ref::<std::io::Error>())
        .map(|io| matches!(io.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock))
        .unwrap_or(false)
        || msg.contains("timed out");
    if timed_out
    {
        AgentError::Timeout
    }
    else
    {
        AgentError::Unreachable(msg)
    }
}

#[cfg(feature = "ollama")]
impl Agent for OllamaAgent
{
    // Architecture §11 R4(説明可能性): provenance に残る名前は
    // "ollama:<モデル名>"(例: "ollama:gemma3")。モデル単位で追跡できる。
    fn name(&self) -> &str
    {
        &self.name
    }

    fn ask(&self, question: &str) -> Result<String, AgentError>
    {
        self.chat(question)
    }

    // B経路: 2回目のプロンプトで構造化申告を要求する(設計 §7)。
    // 失敗時は L0ヒューリスティックへフォールバック(declare_or_fallback)。
    fn self_declare(&self, question: &str, answer: &str) -> SelfDeclaration
    {
        let raw = self.chat(&build_declare_prompt(question, answer));
        declare_or_fallback(raw, question)
    }
}
