// 揮発性タグ付与と共有可否ゲート(設計メモ §3, §5 のL0ルール縮小版)。
//
//  - 揮発性: 時間指示語を含む → volatile / それ以外 → slow
//    (permanent昇格は案4=知識グラフ分解が必要なためPoCでは行わない。
//     「疑わしきはslow/volatile側へ」の非対称原則)
//  - 共有可否: 文脈自立(軸1) AND 事実型かつ非volatile(軸2)。
//    デフォルトは「共有しない」(保守的デフォルト)。

pub struct ShareDecision {
    pub shareable: bool,
    pub reason: String,
}

fn find_term<'a>(q: &str, terms: &[&'a str]) -> Option<&'a str> {
    terms.iter().copied().find(|t| q.contains(t))
}

const VOLATILE_TERMS: &[&str] = &[
    "最新", "現在", "今日", "今の", "いま", "今年", "今月", "価格", "株価", "天気", "latest",
    "current", "today", "now", "price", "weather", "2025年", "2026年", "2027年",
];
const CONTEXT_TERMS: &[&str] = &[
    "それ", "その", "これ", "あれ", "さっき", "上記", "前述", "彼", "続けて", "もっと", "次は",
    "変えて", "直して", " it ", " that ", " above ",
];
const SUBJECTIVE_TERMS: &[&str] = &[
    "おすすめ", "べき", "どう思う", "好き", "best", "should", "recommend", "opinion",
];
const PERSONAL_TERMS: &[&str] = &[
    "私の", "自分の", "俺の", "僕の", "うちの", "このファイル", "このコード", "my ", "our ",
];

// L0ルール: 時間指示語 → volatile / それ以外 → slow
pub fn classify_volatility(question: &str) -> String {
    if find_term(question, VOLATILE_TERMS).is_some() {
        "volatile".to_string()
    } else {
        "slow".to_string()
    }
}

// 共有可否ANDゲート。全チェック通過時のみ共有可。
pub fn share_gate(question: &str, volatility: &str) -> ShareDecision {
    if let Some(t) = find_term(question, CONTEXT_TERMS) {
        return ShareDecision {
            shareable: false,
            reason: format!("文脈依存語を含む: '{t}'"),
        };
    }
    if let Some(t) = find_term(question, SUBJECTIVE_TERMS) {
        return ShareDecision {
            shareable: false,
            reason: format!("主観・意見語を含む: '{t}'"),
        };
    }
    if let Some(t) = find_term(question, PERSONAL_TERMS) {
        return ShareDecision {
            shareable: false,
            reason: format!("個人参照を含む: '{t}'"),
        };
    }
    if volatility == "volatile" {
        return ShareDecision {
            shareable: false,
            reason: "volatile(時事)のためローカル短期TTLのみ".to_string(),
        };
    }
    ShareDecision {
        shareable: true,
        reason: "文脈自立 かつ 非volatile: 共有可".to_string(),
    }
}
