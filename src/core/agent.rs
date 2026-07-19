// Agent層。キャッシュミス時の推論委譲先を抽象化する
// (選択可能な推論先の設計 docs/superpowers/specs/2026-07-18-selectable-inference-backend-design.md。
//  以下「設計 §N」で参照)。
//
//  MockAgent (既定): 固定知識ベース + 汎用フォールバック。ネット不要。
//  OllamaAgent (feature = "ollama"): ローカルOllamaデーモンへのHTTP推論
//  (agent/ollama_agent.rs)。バックエンドは AgentConfig(環境変数)で選択する(設計 §5)。

use crate::volatility::{find_context_term, find_personal_term, find_subjective_term, find_time_term};

// Ollama バックエンド(設計 §6)。純関数(build_chat_request / extract_answer /
// parse_self_declaration 等)は feature なしでもコンパイル・テストされる。
// OllamaAgent 本体(HTTP I/O)のみ feature "ollama" 配下。
pub mod ollama_agent;
#[cfg(feature = "ollama")]
pub use ollama_agent::OllamaAgent;

// L2 Agent自己申告(Architecture §7.3, §5)。
// これは「決定でなく一票」: 申告が No/volatile 側なら共有を止められるが、
// Yes/permanent 側の申告が L0 や案4 の判定を覆すことはない
// (信頼できないLLM申告を安全側にのみ反映する。§10.1 ルール4)。
#[derive(Debug, Clone)]
pub struct SelfDeclaration
{
    pub context_independent: bool, // 前提会話なしで単独回答できるか(軸1)
    pub factual: bool,             // 事実型か(軸2。主観・生成文なら false)
    pub volatility: String,        // 申告volatility: permanent | slow | volatile
}

// 推論失敗の型(設計 §4)。実LLMは失敗しうる(未起動 / 未pull / タイムアウト)ため、
// 失敗を型で表現し、呼び出し側がハンドリング(登録中止 / HTTPエラー化)を選べるようにする。
#[derive(Debug)]
pub enum AgentError
{
    // 推論デーモンに到達できない(未起動 / エンドポイント誤り)
    Unreachable(String),
    // HTTP は通ったが非 2xx(例: モデル未pull)
    Http
    {
        status: u16,
        body: String,
    },
    // タイムアウト
    Timeout,
    // レスポンス JSON のパース失敗
    Parse(String),
}

impl std::fmt::Display for AgentError
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result
    {
        match self
        {
            AgentError::Unreachable(msg) => write!(f, "推論先に到達できない: {msg}"),
            AgentError::Http { status, body } =>
            {
                // body は長大になりうるので先頭のみ(観測用途には十分)
                let head: String = body.chars().take(200).collect();
                write!(f, "推論先がHTTPエラーを返した: status={status} body={head}")
            }
            AgentError::Timeout => write!(f, "推論先がタイムアウトした"),
            AgentError::Parse(msg) => write!(f, "推論レスポンスのパース失敗: {msg}"),
        }
    }
}

impl std::error::Error for AgentError {}

// Send + Sync 上限: S3 でノードデーモンが Arc<dyn Agent> をスレッド間で共有するため。
pub trait Agent: Send + Sync
{
    fn name(&self) -> &str;
    // 推論(設計 §4: Result 化。Mock は常に Ok / 実LLMは失敗しうる)。
    // 同期のまま(trait に async を波及させない。デーモンは spawn_blocking 内から呼ぶ)。
    fn ask(&self, question: &str) -> Result<String, AgentError>;
    // 回答生成後に呼ばれる自己申告(Architecture §7.3 のL2ゲート入力)。
    fn self_declare(&self, question: &str, answer: &str) -> SelfDeclaration;
}

// ------------------------------------------------------------------
// L0ヒューリスティック申告(共通ヘルパー)
// ------------------------------------------------------------------

// L0語彙ルールによるヒューリスティック自己申告。
// MockAgent の申告と OllamaAgent の申告フォールバック(設計 §7: 構造化申告の
// 取得失敗時)が共有する(重複実装を作らない)。
//
//   generated_answer: 回答が「知識ベースにない生成文」であることが呼び出し側で
//   既知の場合 true(MockAgent のフォールバック回答)。true なら「事実型でない」と
//   申告する。実LLM経路では判別できないため false を渡し、生成文の検知は
//   案4(トリプル分解。pipeline)に委ねる。
//
// 揮発性は permanent を自己申告しない(「確信が持てない」側に倒す。
// 案4で permanent 確定した場合は不一致となり、§10.1 ルール4の確信度低下が
// 観測できる)。申告は一票にすぎず、受け取り側(pipeline / finalize_volatility)が
// 安全側にのみ反映する不変条件は変わらない。
pub fn heuristic_self_declare(question: &str, generated_answer: bool) -> SelfDeclaration
{
    let context_independent = find_context_term(question).is_none();
    // 主観質問・個人参照は「事実型でない」と申告する
    // (軸2の事実型は「非主観・非個人」を含む。Architecture §7.3)
    let factual = !generated_answer
        && find_subjective_term(question).is_none()
        && find_personal_term(question).is_none();
    // 揮発性申告: 時事語があれば volatile、なければ保守的に slow
    let volatility = if find_time_term(question).is_some()
    {
        "volatile"
    }
    else
    {
        "slow"
    };
    SelfDeclaration
    {
        context_independent,
        factual,
        volatility: volatility.to_string(),
    }
}

// ------------------------------------------------------------------
// MockAgent(既定バックエンド)
// ------------------------------------------------------------------

pub struct MockAgent;

impl Agent for MockAgent
{
    fn name(&self) -> &str
    {
        "mock"
    }

    fn ask(&self, question: &str) -> Result<String, AgentError>
    {
        const KB: &[(&str, &str)] = &[
            (
                "Winny",
                "Winnyは金子勇氏が2002年に開発したP2Pファイル共有ソフトウェアです。\
                 中央サーバーを持たない純粋P2P型で、キャッシュの中継により匿名性を高める設計でした。",
            ),
            (
                "winny",
                "Winnyは金子勇氏が2002年に開発したP2Pファイル共有ソフトウェアです。",
            ),
            (
                "P2P",
                "P2P(Peer-to-Peer)は、中央サーバーを介さずノード同士が対等に直接通信する\
                 ネットワーク方式です。各ノードがクライアントとサーバーの両方の役割を担います。",
            ),
            (
                "首都",
                "日本の首都は東京です。",
            ),
        ];
        for (k, v) in KB
        {
            if question.contains(k)
            {
                return Ok(v.to_string());
            }
        }
        // 設計 §4: Mock は失敗しない(常に Ok)
        Ok(format!("(モック回答) 「{question}」への回答をここでLLMが生成します。"))
    }

    fn self_declare(&self, question: &str, answer: &str) -> SelfDeclaration
    {
        // モックのヒューリスティック申告。実LLM(OllamaAgent)ではモデル自身に
        // 申告させ、失敗時に同じヘルパーへフォールバックする(設計 §7)。
        // KBフォールバック(生成文)は「事実型でない」と申告する。
        heuristic_self_declare(question, answer.starts_with("(モック回答)"))
    }
}

// ------------------------------------------------------------------
// 設定機構(設計 §5「選べる」の実体)
// ------------------------------------------------------------------

pub const ENV_AGENT_BACKEND: &str = "NYLLM_AGENT_BACKEND";
pub const ENV_OLLAMA_MODEL: &str = "NYLLM_OLLAMA_MODEL";
pub const ENV_OLLAMA_ENDPOINT: &str = "NYLLM_OLLAMA_ENDPOINT";
pub const ENV_OLLAMA_TIMEOUT_SECS: &str = "NYLLM_OLLAMA_TIMEOUT_SECS";

// 既定値。モデルは gemma3(Ollama library に実在・広く入手可能・多言語対応。
// 設計 §11 の実在確認で gemma2 から更新: gemma3 は 270M〜27B の各サイズが
// 公開されており、後継として入手性・日本語性能とも上位互換)。
pub const DEFAULT_OLLAMA_MODEL: &str = "gemma3";
pub const DEFAULT_OLLAMA_ENDPOINT: &str = "http://localhost:11434";
pub const DEFAULT_OLLAMA_TIMEOUT_SECS: u64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentBackend
{
    Mock,
    Ollama,
}

impl AgentBackend
{
    pub fn as_str(self) -> &'static str
    {
        match self
        {
            AgentBackend::Mock => "mock",
            AgentBackend::Ollama => "ollama",
        }
    }
}

// 推論先設定(設計 §5)。環境変数から解決する。将来 TOML ローダを被せる場合も
// 同じ構造体を埋める(設計 §5 の薄いローダ余地)。
#[derive(Debug, Clone)]
pub struct AgentConfig
{
    pub backend: AgentBackend,
    pub model: String,
    pub endpoint: String,
    pub timeout_secs: u64,
}

impl Default for AgentConfig
{
    fn default() -> Self
    {
        Self
        {
            backend: AgentBackend::Mock,
            model: DEFAULT_OLLAMA_MODEL.to_string(),
            endpoint: DEFAULT_OLLAMA_ENDPOINT.to_string(),
            timeout_secs: DEFAULT_OLLAMA_TIMEOUT_SECS,
        }
    }
}

impl AgentConfig
{
    // プロセス環境から解決する薄い入口。解決本体は resolve()(キー→値の
    // ソースを注入可能にし、テストがプロセス環境を変異させずに済む構造)。
    pub fn from_env() -> Self
    {
        Self::resolve(|key| std::env::var(key).ok())
    }

    // 環境変数解決の本体(設計 §5)。
    //   - 不正な backend 値は mock フォールバック + 警告(誤設定で起動不能にしない)
    //   - 不正な timeout 値は既定値フォールバック + 警告
    pub fn resolve<F>(get: F) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        let backend = match get(ENV_AGENT_BACKEND)
        {
            None => AgentBackend::Mock,
            Some(raw) => match raw.trim().to_ascii_lowercase().as_str()
            {
                "" | "mock" => AgentBackend::Mock,
                "ollama" => AgentBackend::Ollama,
                other =>
                {
                    eprintln!(
                        "[agent] 警告: {ENV_AGENT_BACKEND}={other} は不明なバックエンド。\
                         mock にフォールバックします(有効値: mock | ollama)"
                    );
                    AgentBackend::Mock
                }
            },
        };
        let model = get(ENV_OLLAMA_MODEL)
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_OLLAMA_MODEL.to_string());
        let endpoint = get(ENV_OLLAMA_ENDPOINT)
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_OLLAMA_ENDPOINT.to_string());
        let timeout_secs = match get(ENV_OLLAMA_TIMEOUT_SECS)
        {
            None => DEFAULT_OLLAMA_TIMEOUT_SECS,
            Some(raw) => raw.trim().parse().unwrap_or_else(|_|
            {
                eprintln!(
                    "[agent] 警告: {ENV_OLLAMA_TIMEOUT_SECS}={raw} は数値でない。\
                     既定値 {DEFAULT_OLLAMA_TIMEOUT_SECS} 秒を使います"
                );
                DEFAULT_OLLAMA_TIMEOUT_SECS
            }),
        };
        Self
        {
            backend,
            model,
            endpoint,
            timeout_secs,
        }
    }
}

// バックエンド選択ファクトリ(設計 §5)。
// feature "ollama" なしで backend=ollama が指定された場合は mock へ
// フォールバック + 警告(誤設定で起動不能にしない、と同方針)。
pub fn create_agent(config: &AgentConfig) -> Box<dyn Agent>
{
    match config.backend
    {
        AgentBackend::Mock => Box::new(MockAgent),
        AgentBackend::Ollama =>
        {
            #[cfg(feature = "ollama")]
            {
                Box::new(OllamaAgent::new(config))
            }
            #[cfg(not(feature = "ollama"))]
            {
                eprintln!(
                    "[agent] 警告: backend=ollama が指定されたが feature \"ollama\" なしで \
                     ビルドされている。mock にフォールバックします(cargo build --features ollama)"
                );
                Box::new(MockAgent)
            }
        }
    }
}
