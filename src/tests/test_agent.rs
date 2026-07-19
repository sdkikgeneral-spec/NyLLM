// Agent層(選択可能な推論先)のテスト(CLAUDE.md 規則4 /
// docs/superpowers/specs/2026-07-18-selectable-inference-backend-design.md §8)。
//
// カバレッジ:
//   - 純関数: build_chat_request / extract_answer / parse_self_declaration
//     (feature "ollama" なしでもコンパイル・実行される。設計 §6「I/Oと分離」)
//   - self_declare B経路の確定ロジック: 妥当な申告は採用 / 失敗はヒューリスティック
//     フォールバック(declare_or_fallback。設計 §7)
//   - 設定機構: 環境変数解決・既定値・不正値フォールバック(設計 §5。
//     resolve() へ値ソースを注入し、プロセス環境を変異させない)
//   - 不変条件: LLM が Yes/permanent を申告しても pipeline / finalize_volatility の
//     判定が安全側から格上げされない(Architecture §10.1 ルール4)
//   - 統合: ライブ Ollama(#[ignore]。cargo test --features ollama -- --ignored)

use crate::agent::ollama_agent::{
    build_chat_request, build_declare_prompt, declare_or_fallback, extract_answer,
    parse_self_declaration,
};
use crate::agent::{
    create_agent, heuristic_self_declare, Agent, AgentBackend, AgentConfig, AgentError,
    MockAgent, SelfDeclaration, DEFAULT_OLLAMA_ENDPOINT, DEFAULT_OLLAMA_MODEL,
    DEFAULT_OLLAMA_TIMEOUT_SECS, ENV_AGENT_BACKEND, ENV_OLLAMA_ENDPOINT, ENV_OLLAMA_MODEL,
    ENV_OLLAMA_TIMEOUT_SECS,
};
use crate::pipeline::{judge_entry, PipelineStage};
use std::collections::HashMap;

// ------------------------------------------------------------------
// 純関数: build_chat_request / extract_answer(設計 §6)
// ------------------------------------------------------------------

#[test]
fn build_chat_request_produces_expected_json()
{
    let req = build_chat_request("gemma3", "日本の首都はどこですか");
    assert_eq!(req["model"], "gemma3");
    assert_eq!(req["stream"], false, "stream:false(1リクエスト1レスポンス)");
    assert_eq!(req["messages"].as_array().unwrap().len(), 1);
    assert_eq!(req["messages"][0]["role"], "user");
    assert_eq!(req["messages"][0]["content"], "日本の首都はどこですか");
}

#[test]
fn extract_answer_reads_message_content()
{
    let body = r#"{"model":"gemma3","message":{"role":"assistant","content":"東京です。"},"done":true}"#;
    assert_eq!(extract_answer(body).unwrap(), "東京です。");
}

#[test]
fn extract_answer_rejects_non_json_body()
{
    let r = extract_answer("これはJSONではない");
    assert!(matches!(r, Err(AgentError::Parse(_))), "非JSONは Parse: {r:?}");
}

#[test]
fn extract_answer_rejects_missing_content()
{
    // JSONだが .message.content がない(壊れた/想定外のレスポンス形)
    let r = extract_answer(r#"{"model":"gemma3","done":true}"#);
    assert!(matches!(r, Err(AgentError::Parse(_))), ".message.content 欠落は Parse: {r:?}");
    // content が文字列でない場合も Parse
    let r2 = extract_answer(r#"{"message":{"content":123}}"#);
    assert!(matches!(r2, Err(AgentError::Parse(_))), "content 非文字列は Parse: {r2:?}");
}

// ------------------------------------------------------------------
// 純関数: parse_self_declaration(設計 §7)
// ------------------------------------------------------------------

#[test]
fn parse_self_declaration_accepts_valid_json()
{
    let d = parse_self_declaration(
        r#"{"context_independent": true, "factual": true, "volatility": "slow"}"#,
    )
    .expect("妥当な申告JSONはパースできる");
    assert!(d.context_independent);
    assert!(d.factual);
    assert_eq!(d.volatility, "slow");
}

#[test]
fn parse_self_declaration_accepts_fenced_json()
{
    // モデルがコードフェンスや説明文で包んでも、最初の { 〜 最後の } を切り出す
    let raw = "申告は以下です:\n```json\n{\"context_independent\": false, \"factual\": true, \"volatility\": \"volatile\"}\n```";
    let d = parse_self_declaration(raw).expect("フェンス付きでもパースできる");
    assert!(!d.context_independent);
    assert_eq!(d.volatility, "volatile");
}

#[test]
fn parse_self_declaration_rejects_broken_input()
{
    assert!(parse_self_declaration("回答できません").is_none(), "JSONなし");
    assert!(parse_self_declaration("{不正なJSON}").is_none(), "壊れたJSON");
    assert!(
        parse_self_declaration(r#"{"factual": true, "volatility": "slow"}"#).is_none(),
        "必須キー欠落(context_independent なし)"
    );
    assert!(
        parse_self_declaration(
            r#"{"context_independent": "yes", "factual": true, "volatility": "slow"}"#
        )
        .is_none(),
        "bool でない値"
    );
}

#[test]
fn parse_self_declaration_rejects_invalid_volatility()
{
    assert!(
        parse_self_declaration(
            r#"{"context_independent": true, "factual": true, "volatility": "eternal"}"#
        )
        .is_none(),
        "不正な volatility 値は不採用(permanent|slow|volatile のみ)"
    );
}

#[test]
fn declare_prompt_contains_question_answer_and_schema()
{
    let p = build_declare_prompt("日本の首都はどこですか", "東京です。");
    assert!(p.contains("日本の首都はどこですか"));
    assert!(p.contains("東京です。"));
    assert!(p.contains("context_independent") && p.contains("volatility"));
}

// ------------------------------------------------------------------
// self_declare B経路の確定: 採用 / フォールバック(設計 §7)
// ------------------------------------------------------------------

#[test]
fn declare_or_fallback_adopts_valid_declaration()
{
    let raw = Ok(r#"{"context_independent": false, "factual": false, "volatility": "volatile"}"#
        .to_string());
    let d = declare_or_fallback(raw, "日本の首都はどこですか");
    // ヒューリスティックなら (true, true, slow) になる質問 → 申告採用を区別できる
    assert!(!d.context_independent, "妥当な申告JSONはそのまま採用される");
    assert!(!d.factual);
    assert_eq!(d.volatility, "volatile");
}

#[test]
fn declare_or_fallback_falls_back_on_http_error()
{
    let raw: Result<String, AgentError> = Err(AgentError::Timeout);
    let d = declare_or_fallback(raw, "日本の首都はどこですか");
    // フォールバック = L0ヒューリスティック(時事語なし → slow / 文脈語なし → 自立)
    assert!(d.context_independent);
    assert!(d.factual);
    assert_eq!(d.volatility, "slow");
}

#[test]
fn declare_or_fallback_falls_back_on_invalid_volatility()
{
    let raw = Ok(r#"{"context_independent": true, "factual": true, "volatility": "forever"}"#
        .to_string());
    let d = declare_or_fallback(raw, "今日の東京の天気はどうですか");
    // 不正値 → フォールバック。時事語(今日/天気)があるので volatile を申告する
    assert_eq!(d.volatility, "volatile", "不正 volatility 値はヒューリスティックへ");
}

#[test]
fn declare_or_fallback_falls_back_on_unparsable_body()
{
    let raw = Ok("わかりません".to_string());
    let d = declare_or_fallback(raw, "日本の首都はどこですか");
    assert_eq!(d.volatility, "slow", "パース不能はヒューリスティックへ");
}

// ------------------------------------------------------------------
// L0ヒューリスティック申告(共通ヘルパー)
// ------------------------------------------------------------------

#[test]
fn heuristic_declares_safely()
{
    // 文脈依存語 → 単独回答不可
    assert!(!heuristic_self_declare("それを続けて", false).context_independent);
    // 主観語 → 非事実型
    assert!(!heuristic_self_declare("おすすめの本は", false).factual);
    // 個人参照 → 非事実型
    assert!(!heuristic_self_declare("私のコードを直して", false).factual);
    // 生成文フラグ → 非事実型(MockAgent のKBフォールバック相当)
    assert!(!heuristic_self_declare("量子コンピュータとは", true).factual);
    // 時事語 → volatile / なければ slow。permanent は決して自己申告しない
    assert_eq!(heuristic_self_declare("今日の天気は", false).volatility, "volatile");
    assert_eq!(heuristic_self_declare("日本の首都は", false).volatility, "slow");
}

// ------------------------------------------------------------------
// 設定機構(設計 §5。resolve() に値ソースを注入)
// ------------------------------------------------------------------

fn config_from(pairs: &[(&str, &str)]) -> AgentConfig
{
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    AgentConfig::resolve(|key| map.get(key).cloned())
}

#[test]
fn config_defaults_when_env_is_empty()
{
    let c = config_from(&[]);
    assert_eq!(c.backend, AgentBackend::Mock, "既定バックエンドは mock");
    assert_eq!(c.model, DEFAULT_OLLAMA_MODEL);
    assert_eq!(c.endpoint, DEFAULT_OLLAMA_ENDPOINT);
    assert_eq!(c.timeout_secs, DEFAULT_OLLAMA_TIMEOUT_SECS);
}

#[test]
fn config_resolves_ollama_backend_and_overrides()
{
    let c = config_from(&[
        (ENV_AGENT_BACKEND, "ollama"),
        (ENV_OLLAMA_MODEL, "glm4"),
        (ENV_OLLAMA_ENDPOINT, "http://10.0.0.5:11434"),
        (ENV_OLLAMA_TIMEOUT_SECS, "5"),
    ]);
    assert_eq!(c.backend, AgentBackend::Ollama);
    assert_eq!(c.model, "glm4");
    assert_eq!(c.endpoint, "http://10.0.0.5:11434");
    assert_eq!(c.timeout_secs, 5);
}

#[test]
fn config_backend_is_case_insensitive()
{
    let c = config_from(&[(ENV_AGENT_BACKEND, " Ollama ")]);
    assert_eq!(c.backend, AgentBackend::Ollama, "大文字小文字・前後空白は吸収する");
}

#[test]
fn config_falls_back_to_mock_on_invalid_backend()
{
    let c = config_from(&[(ENV_AGENT_BACKEND, "chatgpt")]);
    assert_eq!(c.backend, AgentBackend::Mock, "不正 backend 値は mock フォールバック");
}

#[test]
fn config_falls_back_to_default_on_invalid_timeout()
{
    let c = config_from(&[(ENV_OLLAMA_TIMEOUT_SECS, "abc")]);
    assert_eq!(c.timeout_secs, DEFAULT_OLLAMA_TIMEOUT_SECS, "不正 timeout は既定値");
}

#[test]
fn create_agent_returns_mock_for_mock_backend()
{
    let agent = create_agent(&AgentConfig::default());
    assert_eq!(agent.name(), "mock");
    // 設計 §4: Mock の ask は常に Ok
    assert!(agent.ask("何でも").is_ok());
}

// feature なしビルド: backend=ollama は mock フォールバック(起動不能にしない)
#[cfg(not(feature = "ollama"))]
#[test]
fn create_agent_falls_back_to_mock_without_ollama_feature()
{
    let mut c = AgentConfig::default();
    c.backend = AgentBackend::Ollama;
    let agent = create_agent(&c);
    assert_eq!(agent.name(), "mock", "feature なしでは mock フォールバック");
}

// feature ありビルド: backend=ollama で OllamaAgent が生成される
#[cfg(feature = "ollama")]
#[test]
fn create_agent_returns_ollama_with_feature()
{
    let mut c = AgentConfig::default();
    c.backend = AgentBackend::Ollama;
    let agent = create_agent(&c);
    assert_eq!(agent.name(), format!("ollama:{DEFAULT_OLLAMA_MODEL}"));
}

// Architecture §11 R4(説明可能性): provenance に記録される Agent 名は
// "ollama:<モデル名>" 形式で、どのモデルが生成したかを追跡できること。
#[cfg(feature = "ollama")]
#[test]
fn ollama_agent_name_includes_model_for_provenance()
{
    use crate::agent::OllamaAgent;
    let mut c = AgentConfig::default();
    c.backend = AgentBackend::Ollama;
    c.model = "glm4".to_string();
    let agent = OllamaAgent::new(&c);
    assert_eq!(agent.name(), "ollama:glm4", "provenance はモデル単位で追跡可能");
}

// ------------------------------------------------------------------
// MockAgent の Result 化(設計 §4)
// ------------------------------------------------------------------

#[test]
fn mock_agent_ask_is_always_ok()
{
    let a = MockAgent;
    assert!(a.ask("日本の首都はどこですか").unwrap().contains("東京"));
    assert!(
        a.ask("知らない質問").unwrap().starts_with("(モック回答)"),
        "KB外はフォールバック生成文(それでも Ok)"
    );
}

// ------------------------------------------------------------------
// 不変条件: Yes/permanent 側の申告は判定を覆さない(§10.1 ルール4 /
// 設計 §7 手順4。受け取り側 = pipeline / finalize_volatility のテスト)
// ------------------------------------------------------------------

// 常に「単独回答可・事実型・permanent」を過大申告するテスト用 Agent。
struct OverclaimingAgent;

impl Agent for OverclaimingAgent
{
    fn name(&self) -> &str
    {
        "overclaiming(テスト専用)"
    }

    fn ask(&self, _question: &str) -> Result<String, AgentError>
    {
        Ok("東京の天気は晴れです。".to_string())
    }

    fn self_declare(&self, _question: &str, _answer: &str) -> SelfDeclaration
    {
        SelfDeclaration
        {
            context_independent: true,
            factual: true,
            volatility: "permanent".to_string(),
        }
    }
}

#[test]
fn permanent_declaration_does_not_override_forced_volatile()
{
    // 時間指示語を含む質問 → §10.1 ルール2の強制 volatile。
    // Agent が permanent/Yes を申告しても、クラスは volatile のまま・共有不可のまま。
    let question = "今日の東京の天気はどうですか";
    let answer = "東京の天気は晴れです。";
    let report = judge_entry(question, answer, &OverclaimingAgent);
    assert_eq!(
        report.volatility.class, "volatile",
        "permanent 申告は強制 volatile を覆さない(ルール4)"
    );
    assert!(!report.shareable, "volatile は共有不可のまま");
    assert_eq!(
        report.blocked_at,
        Some(PipelineStage::L0Lexical),
        "L0(時事語)で足切りされる"
    );
}

#[test]
fn permanent_declaration_does_not_promote_undecomposable_answer()
{
    // 分解不能な生成文(§10.1 ルール3 → slow)に対する permanent 申告は、
    // クラスを動かさず確信度を下げるだけ(不一致ペナルティ)。
    struct VagueOverclaimer;
    impl Agent for VagueOverclaimer
    {
        fn name(&self) -> &str
        {
            "vague(テスト専用)"
        }
        fn ask(&self, _q: &str) -> Result<String, AgentError>
        {
            Ok("うーん、なんとも言えませんね。".to_string())
        }
        fn self_declare(&self, _q: &str, _a: &str) -> SelfDeclaration
        {
            SelfDeclaration
            {
                context_independent: true,
                factual: true,
                volatility: "permanent".to_string(),
            }
        }
    }
    let question = "宇宙の意味を教えてください";
    let answer = "うーん、なんとも言えませんね。";
    let report = judge_entry(question, answer, &VagueOverclaimer);
    assert_eq!(
        report.volatility.class, "slow",
        "分解失敗のデフォルト slow を permanent 申告で格上げしない(ルール4)"
    );
    assert!(
        report.volatility.evidence.iter().any(|e| e.contains("self_report_mismatch")),
        "不一致は確信度低下の根拠として記録される: {:?}",
        report.volatility.evidence
    );
    assert!(!report.shareable, "分解不能な生成文は共有不可(案4)");
}

// ------------------------------------------------------------------
// 脅威レビュー 2026-07-19 M-1: 全許可 self_declare(プロンプトインジェクションで
// L2 申告を細工された想定)でも、LLM 非依存の独立バリア
// (L0 語彙 / 案4 トリプル分解 / 確定 volatility)は突破できない。
//
// 上の不変条件テスト2件は「permanent 申告が揮発性クラスを覆せない」
// (§10.1 ルール4。クラス/確信度の観点)を固定するのに対し、本節は
// 「共有をブロックする段(blocked_at)が L2 申告に依存しない」ことを固定する。
// L2 は実LLM経路では実効バリアと数えない(pipeline.rs の M-1 コメント参照)。
// Agent スタブは既存の OverclaimingAgent(全許可申告)を流用する。
// ------------------------------------------------------------------

#[test]
fn all_allow_declaration_cannot_bypass_l0_lexical_gate()
{
    // 回答側は「他の全段がグリーン」になる形(分解可能・既知 permanent 述語・
    // 時事語なし)に整え、質問の L0 語彙だけが唯一のブロック要因である状況を作る。
    let answer = "PostgreSQLの開発者はコミュニティです。";

    // (a-1) 主観語(おすすめ)を含む質問 → 全許可申告でも L0 で足切り
    let report = judge_entry("おすすめのデータベースは何ですか", answer, &OverclaimingAgent);
    assert!(
        report.declaration.context_independent && report.declaration.factual,
        "前提: L2 申告自体は全許可(インジェクション成功の想定)"
    );
    assert!(
        report.decomposition.success && report.volatility.class == "permanent",
        "前提: 案4・確定volatility は通過しうる回答である"
    );
    assert!(!report.shareable, "主観語を含む質問は全許可申告でも共有不可のまま");
    assert_eq!(
        report.blocked_at,
        Some(PipelineStage::L0Lexical),
        "L0(主観語)が L2 申告と無関係に独立バリアとして効く"
    );

    // (a-2) 文脈依存語(その)を含む質問 → 同様に L0 で足切り
    let report = judge_entry("その開発者は誰ですか", answer, &OverclaimingAgent);
    assert!(!report.shareable, "文脈依存語を含む質問は全許可申告でも共有不可のまま");
    assert_eq!(
        report.blocked_at,
        Some(PipelineStage::L0Lexical),
        "L0(文脈依存語)が L2 申告と無関係に独立バリアとして効く"
    );
}

#[test]
fn all_allow_declaration_cannot_bypass_triple_decomposition()
{
    // 質問は L0 を通る中立な語彙にし、回答をトリプル分解不能な生成文にする。
    // 全許可申告で L2 を素通りしても、案4 が独立にブロックすることを固定する
    // (既存テストは分解失敗時の揮発性クラス(slow 維持)を見る。こちらは blocked_at)。
    let question = "Rustはどんな言語ですか";
    let answer = "とても表現力が高く、安心して書けますよ。";
    let report = judge_entry(question, answer, &OverclaimingAgent);
    assert!(report.l0_gate.shareable, "前提: 質問は L0 を通過する");
    assert!(
        report.declaration.context_independent && report.declaration.factual,
        "前提: L2 申告自体は全許可(インジェクション成功の想定)"
    );
    assert!(!report.decomposition.success, "前提: 回答は分解不能な生成文");
    assert!(!report.shareable, "分解不能な回答は全許可申告でも共有不可のまま");
    assert_eq!(
        report.blocked_at,
        Some(PipelineStage::TripleDecomposition),
        "案4 が L2 申告と無関係に独立バリアとして効く"
    );
}

#[test]
fn all_allow_declaration_cannot_bypass_final_volatility()
{
    // 質問側に時事語があるケースは L0 で足切りされる(上の
    // permanent_declaration_does_not_override_forced_volatile で固定済み)ため、
    // ここでは質問を中立に保って L0 を通し、回答側の時事語による強制 volatile
    // (§10.1 ルール2 回答側走査。脅威レビュー Medium-1 対応経路)が
    // FinalVolatility 段の独立バリアとして効くことを固定する
    // (= 細工入力で質問をクリーンに保っても permanent 申告で共有可にできない)。
    let question = "ビットコインとは何ですか";
    let answer = "ビットコインの価格は上昇中です。";
    let report = judge_entry(question, answer, &OverclaimingAgent);
    assert!(report.l0_gate.shareable, "前提: 質問は L0 を通過する");
    assert!(
        report.decomposition.success && report.decomposition.fully_decomposed,
        "前提: 案4(分解・allowlist)も通過する回答である"
    );
    assert_eq!(
        report.volatility.class, "volatile",
        "permanent 全許可申告でも回答側時事語の強制 volatile は覆らない"
    );
    assert!(
        report.volatility.evidence.iter().any(|e| e.contains("time_term_answer")),
        "強制 volatile の根拠(回答側時事語)が evidence に残る: {:?}",
        report.volatility.evidence
    );
    assert!(!report.shareable, "volatile 確定は全許可申告でも共有不可のまま");
    assert_eq!(
        report.blocked_at,
        Some(PipelineStage::FinalVolatility),
        "確定 volatility が L2 申告と無関係に独立バリアとして効く"
    );
}

// ------------------------------------------------------------------
// 統合: ライブ Ollama(設計 §8。ローカルで Ollama 起動 + モデル pull 済みが前提)
//   cargo test --features ollama -- --ignored --nocapture live_ollama
// ------------------------------------------------------------------

#[cfg(feature = "ollama")]
#[test]
#[ignore]
fn live_ollama_ask_and_self_declare()
{
    use crate::agent::OllamaAgent;
    let mut config = AgentConfig::from_env(); // モデル等は環境変数で上書き可能
    config.backend = AgentBackend::Ollama;
    let agent = OllamaAgent::new(&config);

    let question = "日本の首都はどこですか。一文で答えてください。";
    let answer = agent.ask(question).expect("ライブ Ollama への ask が失敗");
    assert!(!answer.trim().is_empty(), "回答が空でない");
    println!("[live] answer: {answer}");

    let d = agent.self_declare(question, &answer);
    assert!(
        matches!(d.volatility.as_str(), "permanent" | "slow" | "volatile"),
        "申告(またはフォールバック)の volatility は妥当な値: {d:?}"
    );
    println!("[live] declaration: {d:?}");
}
