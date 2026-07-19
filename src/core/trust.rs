// S4 層1 内在信頼度 — 算出コア(純粋関数)。
// docs/S4_Company_Phase1_層1内在信頼度先行設計.md §1・§3・§9-1。
//
// 同一 question_key の版集合(独立生成された複数回答とみなす)の facts
// (案4トリプル分解の出力)を正規化済み (s, p, o) 集合とみなし、
// 版の全ペア間 Jaccard 係数の平均を independent_agreement とする(案A)。
//
// 本モジュールは意図的に「純粋関数のみ」で構成する(§8 テスト観点の決定性・
// 一致→高値/矛盾→低下を、キャッシュ・I/O・ポリシー実装から切り離して
// 単体検証できるようにするため):
//   - 入力 = 版集合の facts(スライスのスライス)
//   - 出力 = Trust { independent_agreement, supporting_versions }
//   - I/O・時刻・乱数・グローバル状態に一切依存しない
//
// 差し替え可能性(§7 policy hook 化): 実運用パスは policy::TrustPolicy
// (5点目の差し替え点)経由で本関数を呼ぶ。Phase2 は TrustPolicy 実装を
// 「witness独立性検証つき一致率」へ差し替えるが、本算出コア(Jaccard平均)
// 自体はその実装からも再利用できる形に保つ。
//
// 層1は「決定」ではなく「助言」である(§0)。本モジュールの出力は検索ランキング
// のタイブレークと UI 表示にのみ使われ、共有ゲート(shareable)には配線しない。

use crate::entry::{fold, Trust};
use crate::triples::FactTriple;
use std::collections::BTreeSet;

// 正規化済みトリプル。fold(NFC → 小文字化 → trim → 連続空白畳み込み。
// entry.rs の question_key 用正規化と同一関数)を各要素に適用したもの。
// 表記ゆらぎ(大文字小文字・空白差)だけで「不一致」に倒れることを防ぐ。
pub type NormalizedTriple = (String, String, String);

// facts を正規化済みトリプル集合へ変換する(重複トリプルは集合として畳む)。
pub fn normalized_fact_set(facts: &[FactTriple]) -> BTreeSet<NormalizedTriple>
{
    facts
        .iter()
        .map(|t| (fold(&t.s), fold(&t.p), fold(&t.o)))
        .collect()
}

// Jaccard 係数 = |交差| / |和集合|(0..1)。両方空の場合は 0.0
// (呼び出し側 compute_layer1_trust は空 facts の版を事前に除外するため、
//  通常この分岐は通らない防御枝)。
pub fn jaccard(a: &BTreeSet<NormalizedTriple>, b: &BTreeSet<NormalizedTriple>) -> f64
{
    let inter = a.intersection(b).count();
    let union = a.union(b).count();
    if union == 0
    {
        return 0.0;
    }
    inter as f64 / union as f64
}

// 層1 trust の算出(案A: 版ペア間 Jaccard 平均。§3・§9-1)。
//
//   - 入力: 同一 question_key バンドル各版の facts。到達順は問わない
//     (内部で正準順にソートするため、版集合が同じ多重集合なら
//      入力順によらず浮動小数の加算順序まで同一 = 決定的。§8「決定性」)。
//   - facts が空の版(分解不能相当)は集計から除外する。
//     supporting_versions = facts 分解成功版数(除外後の版数)。
//   - 版ペアが存在しない場合(成功版が 0 or 1)は independent_agreement = 0.0
//     とする。「独立した裏づけがまだ無い」状態を高一致(1.0)と区別する
//     保守側の規約(層1は助言のみのため、低く出る方向の誤りは実害がない。
//     §6 非単調性と同じく「疑わしきは低く」)。単独版であることは
//     supporting_versions = 1 が別途伝える。
pub fn compute_layer1_trust(version_facts: &[&[FactTriple]]) -> Trust
{
    // 分解成功版のみ集計対象にする(§3: supporting_versions の定義)
    let mut sets: Vec<BTreeSet<NormalizedTriple>> = version_facts
        .iter()
        .filter(|f| !f.is_empty())
        .map(|f| normalized_fact_set(f))
        .collect();

    // 決定性の担保: 版の到達順(呼び出し側の保持順)に依存しないよう、
    // 集合の正準順(BTreeSet の辞書式 Ord)でソートしてからペアを走査する。
    // これで各ノードが異なる順序で版を受信しても算出値はビット一致する。
    sets.sort();

    let n = sets.len();
    let mut sum = 0.0f64;
    let mut pairs = 0usize;
    for i in 0..n
    {
        for j in (i + 1)..n
        {
            sum += jaccard(&sets[i], &sets[j]);
            pairs += 1;
        }
    }
    let independent_agreement = if pairs == 0
    {
        0.0 // 成功版 0 or 1 = ペア不在(上記コメントの保守側規約)
    }
    else
    {
        sum / pairs as f64
    };

    Trust
    {
        independent_agreement,
        supporting_versions: n as u32,
    }
}

// 検索ランキングのタイブレーク比較(純粋関数。§4・§9-2)。
//
// 類似度が同点の候補 cand が現在の最良 best を置き換えるべきなら true。
// 選好順(§9-2 採用案: created 新しい順が主軸、trust はタイブレーク):
//   1. created が新しい方(S3 §4 の既存選好。従来挙動と同一)
//   2. created も同点のとき、実測ゲート有効(trust_weight > 0)なら
//      trust_weight * independent_agreement が大きい方
//   3. それも同点なら supporting_versions が多い方
//
// 実測ゲート(§4): trust_weight <= 0.0(既定)では手順2以降を一切評価しない
// ため、trust 値がどうであれ従来のランキング(created タイブレークのみ)と
// 完全に同一の順位になる(§8「実測ゲート無効時の重み0」の実装点)。
// trust 未算出(None)の版は agreement 0.0 / versions 0 として比較する。
pub fn prefer_candidate(
    cand_created: &str,
    cand_trust: Option<&Trust>,
    best_created: &str,
    best_trust: Option<&Trust>,
    trust_weight: f64,
) -> bool
{
    // 主軸: created 新しい順(RFC3339・Z終端固定なので文字列比較で時刻順)
    if cand_created != best_created
    {
        return best_created < cand_created;
    }
    // 実測ゲート既定=無効: trust は順位に一切寄与しない(従来挙動と一致)
    if trust_weight <= 0.0
    {
        return false;
    }
    let (ca, cs) = cand_trust
        .map(|t| (t.independent_agreement, t.supporting_versions))
        .unwrap_or((0.0, 0));
    let (ba, bs) = best_trust
        .map(|t| (t.independent_agreement, t.supporting_versions))
        .unwrap_or((0.0, 0));
    let cw = trust_weight * ca;
    let bw = trust_weight * ba;
    if cw != bw
    {
        return cw > bw;
    }
    cs > bs
}
