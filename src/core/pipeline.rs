// 共有対象の判定パイプライン(Architecture §7)。
//
//   回答生成完了
//     → [L0]   文脈依存語/揮発語/主観語/個人参照(語彙ルール。share_gate)
//     → [L2]   Agent自己申告: 単独回答可? 事実型?(§7.3)
//     → [案4]  回答の事実トリプル分解: 分解不能=生成文 → 共有除外(§7.3)。
//              未解析文が残る/オントロジー未収録述語を含む → 共有除外
//              (脅威レビュー Medium-2: S2 の共有可は収録済み述語のみの allowlist)
//     → [確定] 揮発性初期付与(§10.1)。volatile → 共有除外
//   全段通過時のみ共有可(デフォルト非共有の保守的ANDゲート。§7.1)。
//
// 各段の結果は PipelineReport の pub フィールドとして全て観測可能にする
// (Roadmap S2 ゲート「§7フロー通過率を実測」のための構造。実測・記録は
//  テスト側が行う)。§7.4 の実運用ではL0で足切りして以降の段を省くが、
// 通過率観測のため全段を常に計算し、ブロック段を blocked_at に記録する。

use crate::agent::{Agent, SelfDeclaration};
use crate::triples::{decompose, TripleDecomposition};
use crate::volatility::{
    classify_volatility, finalize_volatility, share_gate, ShareDecision, VolatilityAssessment,
};

// 共有不可が確定した段(§7.4 の判定順)。None なら全段通過=共有可。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineStage
{
    L0Lexical,           // L0 語彙ゲート(文脈依存語/揮発語/主観語/個人参照)
    L2SelfDeclaration,   // L2 自己申告(単独回答不可 または 非事実型)
    TripleDecomposition, // 案4 分解不能(生成文とみなす)/ 未解析文が残る
    UnknownPredicate,    // 案4 オントロジー未収録述語を含む(脅威レビュー Medium-2: allowlist)
    FinalVolatility,     // 確定 volatility が volatile(述語由来)
}

// 判定パイプラインの全段観測レポート。
pub struct PipelineReport
{
    pub l0_volatility: String,             // L0語彙分類(volatile | slow。参考値)
    pub l0_gate: ShareDecision,            // L0のみでの共有可否(S1既存ゲートと同一)
    pub declaration: SelfDeclaration,      // L2 Agent自己申告
    pub decomposition: TripleDecomposition, // 案4 トリプル分解の結果
    pub volatility: VolatilityAssessment,  // §10.1 確定 {class, confidence, evidence}
    pub shareable: bool,                   // 最終共有可否(全段AND)
    pub share_reason: String,              // 最終判定理由
    pub blocked_at: Option<PipelineStage>, // 最初に共有不可が確定した段
}

// 判定パイプラインを1本通す(ミス時=登録時にのみ呼ぶ。§7.2 書き込み経路)。
//
// 共有判定は既存の share_gate(L0)に L2・案4・確定volatility の条件を
// AND で追加する形になっており、S1 のゲートより厳しくなることはあっても
// 緩くなることはない(保守的デフォルトの不変条件を維持)。
pub fn judge_entry(question: &str, answer: &str, agent: &dyn Agent) -> PipelineReport
{
    // --- L0: 語彙ルール(S1既存実装をそのまま利用) ---
    let l0_volatility = classify_volatility(question);
    let l0_gate = share_gate(question, &l0_volatility);

    // --- L2: Agent自己申告(§7.3。決定でなく一票) ---
    //
    // 脅威レビュー 2026-07-19 M-1: Mock 経路の申告は決定的な L0 語彙ルール由来
    // だが、実LLM経路(Ollama B経路)ではこの申告はプロンプトインジェクションで
    // 細工可能な「信頼できない入力」になる。細工入力で全許可申告
    // {context_independent:true, factual:true, volatility:"permanent"} を
    // 返させれば L2 ブロックは素通りするため、L2 を多層防御の実効バリアとして
    // 数えてはならない。共有阻止の独立バリアはあくまで LLM 非依存の
    // L0(語彙)/案4(トリプル分解 allowlist)/確定 volatility の3段であり、
    // 全許可申告でもこれらを通過しない限り共有可にならないことは
    // tests/test_agent.rs の M-1 系テストで固定している。
    let declaration = agent.self_declare(question, answer);

    // --- 案4: 回答のトリプル分解(§7.3, §10.1) ---
    let decomposition = decompose(answer);

    // --- 確定: 揮発性初期付与(§10.1 の4ルール。自己申告は安全側にのみ反映)。
    //     answer も渡す(ルール2の回答側走査。脅威レビュー Medium-1 対応) ---
    let volatility = finalize_volatility(question, answer, &decomposition, &declaration.volatility);

    // --- 共有可否: §7.4 の順で最初のブロック段を確定する ---
    let (blocked_at, share_reason) = if !l0_gate.shareable
    {
        (Some(PipelineStage::L0Lexical), format!("[L0] {}", l0_gate.reason))
    }
    else if !declaration.context_independent
    {
        (
            Some(PipelineStage::L2SelfDeclaration),
            "[L2] 自己申告: 前提会話なしでは単独回答できない".to_string(),
        )
    }
    else if !declaration.factual
    {
        (
            Some(PipelineStage::L2SelfDeclaration),
            "[L2] 自己申告: 事実型でない".to_string(),
        )
    }
    else if !decomposition.success
    {
        (
            Some(PipelineStage::TripleDecomposition),
            "[案4] トリプル分解不能(生成文とみなし共有除外)".to_string(),
        )
    }
    else if !decomposition.fully_decomposed
    {
        // 脅威レビュー Medium-2 対応: 部分的にしか分解できていない回答は
        // 未解析文の内容を検証できないため共有不可(揮発性クラスは変えない)。
        (
            Some(PipelineStage::TripleDecomposition),
            "[案4] 未解析文が残る(分解に完全成功していない)ため共有除外".to_string(),
        )
    }
    else if !decomposition.unknown_predicates.is_empty()
    {
        // 脅威レビュー Medium-2 対応(allowlist): S2 では共有可を
        // オントロジー収録済み述語のみに限定する。未知述語は §10.1 ルール3で
        // slow 分類のまま(ローカル保持は可)だが、「疑わしきは共有しない」
        // (Architecture §7.1)に従い共有フラグのみ保守側に倒す。
        (
            Some(PipelineStage::UnknownPredicate),
            format!(
                "[案4] オントロジー未収録述語({})を含むため共有除外(allowlist)",
                decomposition.unknown_predicates.join(", ")
            ),
        )
    }
    else if volatility.class == "volatile"
    {
        (
            Some(PipelineStage::FinalVolatility),
            "[確定] volatility=volatile のためローカル短期TTLのみ".to_string(),
        )
    }
    else
    {
        (
            None,
            "全ゲート通過(文脈自立 かつ 事実型 かつ 非volatile): 共有可".to_string(),
        )
    };

    PipelineReport
    {
        l0_volatility,
        l0_gate,
        declaration,
        decomposition,
        volatility,
        shareable: blocked_at.is_none(),
        share_reason,
        blocked_at,
    }
}
