// Agent層。キャッシュミス時の推論委譲先を抽象化する。
//
//  MockAgent (既定): 固定知識ベース + 汎用フォールバック。ネット不要。
//  実LLM経路: Agent traitを実装した型(例: Anthropic Claude のMessages APIを
//  HTTPクライアントで叩くClaudeAgent)を差し込む拡張点。PoCではHTTP依存を
//  必須にしないため未実装(python_prototype/agents.py に実装例あり)。

pub trait Agent {
    fn name(&self) -> &str;
    fn ask(&self, question: &str) -> String;
}

pub struct MockAgent;

impl Agent for MockAgent {
    fn name(&self) -> &str {
        "mock"
    }

    fn ask(&self, question: &str) -> String {
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
        ];
        for (k, v) in KB {
            if question.contains(k) {
                return v.to_string();
            }
        }
        format!("(モック回答) 「{question}」への回答をここでLLMが生成します。")
    }
}

pub fn create_agent() -> Box<dyn Agent> {
    Box::new(MockAgent)
}
