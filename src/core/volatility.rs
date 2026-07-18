// 揮発性タグ付与と共有可否ゲート(設計メモ §3, §5 / Architecture §7, §10)。
//
//  - L0(語彙ルール): 時間指示語を含む → volatile / それ以外 → slow
//    (classify_volatility。S1からの既存API。単独では permanent 昇格しない)
//  - 確定(§10.1): finalize_volatility が 案4トリプル分解の結果と
//    L2自己申告を合成して {class, confidence, evidence} を返す。
//    permanent 昇格は「分解成功かつ全述語がpermanent型」の場合のみ
//    (「疑わしきはslow/volatile側へ」の非対称原則)。
//  - 共有可否: 文脈自立(軸1) AND 事実型かつ非volatile(軸2)。
//    デフォルトは「共有しない」(保守的デフォルト)。L0ゲートは share_gate、
//    L2・案4 まで含めた全段判定は pipeline::judge_entry が行う。

use crate::triples::{
    is_definition_predicate, is_timely_object, predicate_class, TripleDecomposition,
    VolatilityClass,
};

pub struct ShareDecision
{
    pub shareable: bool,
    pub reason: String,
}

fn find_term<'a>(q: &str, terms: &[&'a str]) -> Option<&'a str>
{
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

// L0語彙ヘルパー(pub)。L2自己申告のモック実装やパイプラインの観測から
// 同じ語彙表を参照できるようにする(表自体は本モジュールに閉じたまま)。
pub fn find_time_term(text: &str) -> Option<&'static str>
{
    find_term(text, VOLATILE_TERMS)
}
pub fn find_context_term(text: &str) -> Option<&'static str>
{
    find_term(text, CONTEXT_TERMS)
}
pub fn find_subjective_term(text: &str) -> Option<&'static str>
{
    find_term(text, SUBJECTIVE_TERMS)
}
pub fn find_personal_term(text: &str) -> Option<&'static str>
{
    find_term(text, PERSONAL_TERMS)
}

// L0ルール: 時間指示語 → volatile / それ以外 → slow
pub fn classify_volatility(question: &str) -> String
{
    if find_term(question, VOLATILE_TERMS).is_some()
    {
        "volatile".to_string()
    }
    else
    {
        "slow".to_string()
    }
}

// 揮発性の確定結果(Architecture §6 volatility スキーマの縮小版)。
// class は確定クラス、confidence は 0..1 の確信度、evidence は判定根拠
// (§6 の evidence 形式に倣った "predicate_type:permanent" 等の文字列)。
#[derive(Debug, Clone)]
pub struct VolatilityAssessment
{
    pub class: String, // permanent | slow | volatile
    pub confidence: f32,
    pub evidence: Vec<String>,
}

// 確信度の初期値(PoC由来のプレースホルダ。実測チューニングは Roadmap §3)。
const CONF_FORCED_VOLATILE: f32 = 0.9; // ルール2: 時間指示語による強制volatile
const CONF_PERMANENT_INITIAL: f32 = 0.6; // ルール1: §10.1「permanent(confidence:中)」
const CONF_SLOW_KNOWN: f32 = 0.6; // 既知slow型述語による slow
const CONF_SLOW_DEFAULT: f32 = 0.5; // ルール3: 分解失敗/未知述語のデフォルト slow
const CONF_VOLATILE_PREDICATE: f32 = 0.8; // volatile型述語(案4)による volatile
const SELF_REPORT_MISMATCH_FACTOR: f32 = 0.7; // ルール4: 自己申告不一致で確信度を下げる係数
const CONF_FLOOR: f32 = 0.05; // 確信度の下限

// 揮発性の初期付与を確定する(Architecture §10.1「初期ルール(安全側に倒す)」)。
//
//   ルール2: 時間指示語を含む → 強制 volatile(他判定より優先)。
//            質問だけでなく回答(署名対象の answer)も走査する
//            (脅威レビュー Medium-1 対応: 回答側にだけ時事シグナルが
//            現れるケースを取りこぼさない)。
//   ルール1: 分解成功 かつ 全述語が permanent 型 → permanent(confidence: 中)
//   ルール3: 分解失敗/未知述語 → デフォルト slow
//   (案4)   volatile/slow 型述語があれば「最も揮発側」のクラスを採用。
//            コピュラ定義述語(種別/is-a)は目的語が時事シグナル
//            (数値・通貨・年・時点語)を含む場合 permanent とみなさず
//            volatile へ降格する(脅威レビュー Medium-1: 「ビットコインは
//            1000万円です」型の volatile→permanent 洗浄防止)。
//   ルール4: LLM自己申告(declared_volatility)は不一致時に確信度を
//            下げる方向のみ使う。クラスを permanent 側へ動かすことは決してない
//            (自己申告は信頼できない補助信号 — 決定でなく一票)。
//
// 根拠 = 誤分類コストの非対称性(§10.1): volatile→permanent 誤り(毒が居座る)は
// permanent→slow 誤り(早めに再検証)より遥かに危険なので、疑わしきは
// slow/volatile 側へ倒す。
pub fn finalize_volatility(
    question: &str,
    answer: &str,
    decomposition: &TripleDecomposition,
    declared_volatility: &str,
) -> VolatilityAssessment
{
    let mut evidence: Vec<String> = Vec::new();

    let (class, mut confidence) = if let Some(t) = find_time_term(question)
    {
        // ルール2(最優先): 時間指示語 → 強制 volatile
        evidence.push(format!("time_term:{t}"));
        (VolatilityClass::Volatile, CONF_FORCED_VOLATILE)
    }
    else if let Some(t) = find_time_term(answer)
    {
        // ルール2(回答側。脅威レビュー Medium-1 対応): 質問が中立でも
        // 回答に時事シグナルがあれば強制 volatile(署名対象の answer を走査)。
        evidence.push(format!("time_term_answer:{t}"));
        (VolatilityClass::Volatile, CONF_FORCED_VOLATILE)
    }
    else if !decomposition.success
    {
        // ルール3: 分解失敗 → デフォルト slow
        evidence.push("decomposition_failed".to_string());
        (VolatilityClass::Slow, CONF_SLOW_DEFAULT)
    }
    else
    {
        // 案4: 述語クラスの「最大揮発度」を採用。未知述語は slow 扱い(ルール3)。
        let mut max_class = VolatilityClass::Permanent;
        for t in &decomposition.triples
        {
            let mut c = predicate_class(&t.p).unwrap_or(VolatilityClass::Slow);
            // 脅威レビュー Medium-1 対応(目的語形状ガード):
            // コピュラ定義述語(種別/is-a)は目的語を見ずに permanent 型に
            // 固定されるため、目的語が時事シグナルを含む場合は permanent
            // 定義とみなさず volatile へ降格する(疑わしきは volatile 側。§10.1)。
            // facts(署名対象)のトリプル自体は変更せず、昇格判定側で吸収する。
            if c == VolatilityClass::Permanent
                && is_definition_predicate(&t.p)
                && is_timely_object(&t.o)
            {
                evidence.push(format!("timely_definition_object:{}", t.o));
                c = VolatilityClass::Volatile;
            }
            if c > max_class
            {
                max_class = c;
            }
        }
        for p in &decomposition.unknown_predicates
        {
            evidence.push(format!("unknown_predicate:{p}"));
        }
        match max_class
        {
            VolatilityClass::Permanent =>
            {
                // ルール1: 分解成功 かつ 全述語 permanent 型 → permanent(confidence: 中)
                evidence.push("predicate_type:permanent".to_string());
                (VolatilityClass::Permanent, CONF_PERMANENT_INITIAL)
            }
            VolatilityClass::Slow =>
            {
                let conf = if decomposition.unknown_predicates.is_empty()
                {
                    evidence.push("predicate_type:slow".to_string());
                    CONF_SLOW_KNOWN
                }
                else
                {
                    // 未知述語が混在 → ルール3のデフォルト確信度に留める
                    CONF_SLOW_DEFAULT
                };
                (VolatilityClass::Slow, conf)
            }
            VolatilityClass::Volatile =>
            {
                evidence.push("predicate_type:volatile".to_string());
                (VolatilityClass::Volatile, CONF_VOLATILE_PREDICATE)
            }
        }
    };

    // ルール4: 自己申告はクラスを動かさない。不一致なら確信度を下げるだけ
    // (低確信度 = 早期再評価につながるため、下げる方向は常に安全側)。
    if declared_volatility != class.as_str()
    {
        confidence = (confidence * SELF_REPORT_MISMATCH_FACTOR).max(CONF_FLOOR);
        evidence.push(format!("self_report_mismatch(declared={declared_volatility})"));
    }

    VolatilityAssessment
    {
        class: class.as_str().to_string(),
        confidence,
        evidence,
    }
}

// 共有可否ANDゲート。全チェック通過時のみ共有可。
pub fn share_gate(question: &str, volatility: &str) -> ShareDecision
{
    if let Some(t) = find_term(question, CONTEXT_TERMS)
    {
        return ShareDecision
        {
            shareable: false,
            reason: format!("文脈依存語を含む: '{t}'"),
        };
    }
    if let Some(t) = find_term(question, SUBJECTIVE_TERMS)
    {
        return ShareDecision
        {
            shareable: false,
            reason: format!("主観・意見語を含む: '{t}'"),
        };
    }
    if let Some(t) = find_term(question, PERSONAL_TERMS)
    {
        return ShareDecision
        {
            shareable: false,
            reason: format!("個人参照を含む: '{t}'"),
        };
    }
    if volatility == "volatile"
    {
        return ShareDecision
        {
            shareable: false,
            reason: "volatile(時事)のためローカル短期TTLのみ".to_string(),
        };
    }
    ShareDecision
    {
        shareable: true,
        reason: "文脈自立 かつ 非volatile: 共有可".to_string(),
    }
}
