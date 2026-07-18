// Agent層。キャッシュミス時の推論委譲先を抽象化する。
//
//  MockAgent (既定): 固定知識ベース + 汎用フォールバック。ネット不要。
//  実LLM経路: Agent traitを実装した型(例: Anthropic Claude のMessages APIを
//  HTTPクライアントで叩くClaudeAgent)を差し込む拡張点。HTTP依存を
//  必須にしないため未実装(poc/python_prototype/agents.py に実装例あり)。

use crate::volatility::{find_context_term, find_personal_term, find_subjective_term, find_time_term};

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

// Send + Sync 上限: S3 でノードデーモンが Arc<dyn Agent> をスレッド間で共有するため。
pub trait Agent: Send + Sync
{
    fn name(&self) -> &str;
    fn ask(&self, question: &str) -> String;
    // 回答生成後に呼ばれる自己申告(Architecture §7.3 のL2ゲート入力)。
    fn self_declare(&self, question: &str, answer: &str) -> SelfDeclaration;
}

pub struct MockAgent;

impl Agent for MockAgent
{
    fn name(&self) -> &str
    {
        "mock"
    }

    fn ask(&self, question: &str) -> String
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
                return v.to_string();
            }
        }
        format!("(モック回答) 「{question}」への回答をここでLLMが生成します。")
    }

    fn self_declare(&self, question: &str, answer: &str) -> SelfDeclaration
    {
        // モックのヒューリスティック申告。実LLMではモデル自身に申告させる拡張点。
        // L0語彙ヘルパーを流用する(申告が信頼できない前提は変わらないので、
        // 受け取り側=finalize_volatility/pipeline は安全側にのみ使う)。
        let context_independent = find_context_term(question).is_none();
        // KBフォールバック(生成文)・主観質問・個人参照は「事実型でない」と申告する
        // (軸2の事実型は「非主観・非個人」を含む。Architecture §7.3)
        let factual = !answer.starts_with("(モック回答)")
            && find_subjective_term(question).is_none()
            && find_personal_term(question).is_none();
        // 揮発性申告: 時事語があれば volatile、なければ保守的に slow を申告する
        // (permanent は自己申告しない=「確信が持てない」側に倒すモック。
        //  このため案4で permanent 確定した場合は不一致となり、ルール4の
        //  確信度低下がデモでも観測できる)
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
}

pub fn create_agent() -> Box<dyn Agent>
{
    Box::new(MockAgent)
}
