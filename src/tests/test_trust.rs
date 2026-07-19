// S4 層1 内在信頼度のテスト(設計ノート §8「テスト観点」)。
//
// 分業(/goal): 実装は poc-core-dev(fable)、テスト+ベンチは Opus 担当。
// 本ファイルは fable 実装(trust.rs / policy.rs(5点目hook) / cache.rs 再導出 /
// sync.rs フック)を §8 の6観点で検証する:
//   1. 決定性(同一版集合・入力順非依存でビット一致)
//   2. 一致→高値 / 矛盾→低下
//   3. ローカル算出のみ(版facts由来。送信者trust値は構造的に存在しない)
//   4. 実測ゲート無効時の重み0(順位不変)
//   5. policy hook 差し替え(算出コアは不変=Phase2差し替えの予行)
//   6. entry_id / author_sig 不変(trust更新は署名境界に触れない)
//
// 純粋関数(trust.rs)は I/O 非依存で単体検証し、キャッシュ再導出フックは
// verify_envelope + insert_verified の実受信コードパスで検証する。
// マルチノードは §8 どおりプロセス内シミュレーション(common.rs)。
// #[ignore] 付きベンチ(§9-5 hot path 負荷)は末尾。

use super::common::{
    envelope_from_core, make_core, new_signer, shared_embedder, temp_dir, triple,
};
use crate::cache::{SemanticCache, LOCAL_THRESHOLD};
use crate::entry::{question_key, Tier, Trust};
use crate::policy::{Layer1TrustPolicy, TrustPolicy};
use crate::triples::FactTriple;
use crate::trust::{compute_layer1_trust, jaccard, normalized_fact_set, prefer_candidate};
use std::collections::BTreeSet;

// ------------------------------------------------------------------
// 版facts の素材(「日本の首都は東京」を正解版、「大阪」を矛盾版とする)
// ------------------------------------------------------------------

fn facts_tokyo() -> Vec<FactTriple>
{
    vec![triple("日本", "首都", "東京")]
}

fn facts_osaka() -> Vec<FactTriple>
{
    vec![triple("日本", "首都", "大阪")]
}

// ==================================================================
// 純粋関数: jaccard / normalized_fact_set
// ==================================================================

#[test]
fn jaccard_identical_disjoint_partial()
{
    let a: BTreeSet<_> = normalized_fact_set(&facts_tokyo());
    let a2: BTreeSet<_> = normalized_fact_set(&facts_tokyo());
    let b: BTreeSet<_> = normalized_fact_set(&facts_osaka());
    // 同一集合 = 1.0
    assert_eq!(jaccard(&a, &a2), 1.0);
    // 交差なし = 0.0
    assert_eq!(jaccard(&a, &b), 0.0);
    // 部分一致: {t} と {t, o} → 交差1 / 和集合2 = 0.5
    let mut union_facts = facts_tokyo();
    union_facts.extend(facts_osaka());
    let ab: BTreeSet<_> = normalized_fact_set(&union_facts);
    assert_eq!(jaccard(&a, &ab), 0.5);
}

#[test]
fn normalized_fact_set_folds_case_and_whitespace()
{
    // 表記ゆらぎ(大文字小文字・前後空白・連続空白)だけで不一致に倒れないこと。
    let clean = vec![triple("rust", "作者", "graydon hoare")];
    let noisy = vec![triple("  Rust ", "作者", "Graydon   Hoare")];
    let cs: BTreeSet<_> = normalized_fact_set(&clean);
    let ns: BTreeSet<_> = normalized_fact_set(&noisy);
    assert_eq!(cs, ns, "fold(NFC→小文字→trim→空白畳み込み)後は同一トリプルになるべき");
    assert_eq!(jaccard(&cs, &ns), 1.0);
}

// ==================================================================
// 純粋関数: compute_layer1_trust(§8 観点1・2)
// ==================================================================

#[test]
fn compute_all_agree_is_one()
{
    // 2版が完全一致 → independent_agreement = 1.0、supporting = 2(観点2「一致→高値」)
    let v1 = facts_tokyo();
    let v2 = facts_tokyo();
    let t = compute_layer1_trust(&[v1.as_slice(), v2.as_slice()]);
    assert_eq!(t.independent_agreement, 1.0);
    assert_eq!(t.supporting_versions, 2);
}

#[test]
fn compute_conflict_lowers_agreement_monotonically()
{
    // 観点2「矛盾→低下」+ 矛盾版が増えるほど一致率が下がる単調性。
    let tokyo = facts_tokyo();
    let osaka = facts_osaka();

    // [東京, 東京] = 1.0
    let all_agree = compute_layer1_trust(&[tokyo.as_slice(), tokyo.as_slice()]);
    // [東京, 東京, 大阪] = ペア3(1.0, 0.0, 0.0)平均 = 1/3
    let one_conflict =
        compute_layer1_trust(&[tokyo.as_slice(), tokyo.as_slice(), osaka.as_slice()]);
    // [東京, 大阪] = ペア1(0.0)= 0.0
    let split = compute_layer1_trust(&[tokyo.as_slice(), osaka.as_slice()]);

    assert_eq!(all_agree.independent_agreement, 1.0);
    assert!((one_conflict.independent_agreement - (1.0 / 3.0)).abs() < 1e-12);
    assert_eq!(split.independent_agreement, 0.0);
    // 矛盾の混入で単調に低下する
    assert!(all_agree.independent_agreement > one_conflict.independent_agreement);
    assert!(one_conflict.independent_agreement > split.independent_agreement);
    assert_eq!(one_conflict.supporting_versions, 3);
}

#[test]
fn compute_is_deterministic_regardless_of_input_order()
{
    // 観点1「決定性」: 版の到達順が違っても算出値はビット一致する
    // (各ノードが異なる受信順でも同じ trust に収束する=§6 非単調性の抑制)。
    let a = facts_tokyo();
    let b = facts_osaka();
    let mut c = facts_tokyo();
    c.extend(facts_osaka()); // {首都=東京, 首都=大阪}

    let order1 = compute_layer1_trust(&[a.as_slice(), b.as_slice(), c.as_slice()]);
    let order2 = compute_layer1_trust(&[c.as_slice(), a.as_slice(), b.as_slice()]);
    let order3 = compute_layer1_trust(&[b.as_slice(), c.as_slice(), a.as_slice()]);
    // f64 の加算順序まで含めて完全一致(sort による正準化)
    assert_eq!(order1, order2);
    assert_eq!(order2, order3);
}

#[test]
fn compute_no_pair_is_zero_and_empty_versions_excluded()
{
    // 成功版 0/1(ペア不在)→ agreement 0.0(「独立の裏づけ未取得」を高一致1.0と
    // 区別する保守側の規約。fable 実装が明示した規約)。
    let empty: Vec<FactTriple> = vec![];
    let one = facts_tokyo();

    // 版0件
    let t0 = compute_layer1_trust(&[]);
    assert_eq!(t0.independent_agreement, 0.0);
    assert_eq!(t0.supporting_versions, 0);

    // 単独版 → ペア不在 → 0.0、supporting = 1
    let t1 = compute_layer1_trust(&[one.as_slice()]);
    assert_eq!(t1.independent_agreement, 0.0);
    assert_eq!(t1.supporting_versions, 1);

    // facts 空の版は集計対象から除外(supporting に数えない)。
    // [東京, 空, 東京] → 成功版2 → agreement 1.0 / supporting 2
    let t = compute_layer1_trust(&[one.as_slice(), empty.as_slice(), facts_tokyo().as_slice()]);
    assert_eq!(t.supporting_versions, 2);
    assert_eq!(t.independent_agreement, 1.0);
}

// ==================================================================
// 純粋関数: prefer_candidate(§8 観点4「実測ゲート無効時の重み0」)
// ==================================================================

const NEWER: &str = "2026-07-19T00:00:00Z";
const OLDER: &str = "2026-07-10T00:00:00Z";

fn trust(agreement: f64, versions: u32) -> Trust
{
    Trust { independent_agreement: agreement, supporting_versions: versions }
}

#[test]
fn prefer_candidate_weight_zero_ignores_trust()
{
    // 実測ゲート無効(重み0): trust は順位に一切寄与しない。
    let high = trust(1.0, 9);
    let low = trust(0.0, 1);

    // created 新しい候補は(trust が低くても)採用される = created 主軸のまま
    assert!(prefer_candidate(NEWER, Some(&low), OLDER, Some(&high), 0.0));
    // created 同点なら trust 差があっても不採用(順位を動かさない=従来挙動)
    assert!(!prefer_candidate(NEWER, Some(&high), NEWER, Some(&low), 0.0));
    assert!(!prefer_candidate(NEWER, Some(&low), NEWER, Some(&high), 0.0));
}

#[test]
fn prefer_candidate_weight_positive_uses_trust_only_as_tiebreak()
{
    // 実測ゲート有効(重み>0): created 同点のときだけ trust をタイブレークに使う。
    let high = trust(0.9, 3);
    let low = trust(0.1, 3);

    // created 同点 → agreement が高い方を採用
    assert!(prefer_candidate(NEWER, Some(&high), NEWER, Some(&low), 1.0));
    assert!(!prefer_candidate(NEWER, Some(&low), NEWER, Some(&high), 1.0));

    // agreement 同点 → supporting_versions が多い方を採用
    let many = trust(0.5, 5);
    let few = trust(0.5, 2);
    assert!(prefer_candidate(NEWER, Some(&many), NEWER, Some(&few), 1.0));
}

#[test]
fn prefer_candidate_created_dominates_even_with_weight()
{
    // 重み>0 でも created が主軸: 古い高trust版は新しい低trust版に勝てない(§9-2)。
    let high = trust(1.0, 9);
    let low = trust(0.0, 0);
    assert!(prefer_candidate(NEWER, Some(&low), OLDER, Some(&high), 5.0));
    assert!(!prefer_candidate(OLDER, Some(&high), NEWER, Some(&low), 5.0));
}

#[test]
fn prefer_candidate_handles_missing_trust()
{
    // trust 未算出(None)の版は agreement 0.0 / versions 0 として比較する。
    let present = trust(0.8, 2);
    assert!(prefer_candidate(NEWER, Some(&present), NEWER, None, 1.0));
    assert!(!prefer_candidate(NEWER, None, NEWER, Some(&present), 1.0));
}

// ==================================================================
// policy hook 差し替え(§8 観点5)
// ==================================================================

#[test]
fn layer1_policy_delegates_to_core_and_defaults_weight_zero()
{
    // Layer1TrustPolicy::compute は算出コア(compute_layer1_trust)と一致し、
    // 既定 ranking_weight は 0.0(実測ゲート無効)。
    let pol = Layer1TrustPolicy::new();
    let v1 = facts_tokyo();
    let v2 = facts_osaka();
    let via_policy = pol.compute(&[v1.as_slice(), v2.as_slice()]);
    let via_core = compute_layer1_trust(&[v1.as_slice(), v2.as_slice()]);
    assert_eq!(via_policy, via_core);
    assert_eq!(pol.ranking_weight(), 0.0);
}

#[test]
fn layer1_policy_with_weight_changes_only_ranking_not_core()
{
    // 実測ゲートの重みを変えても算出コアは不変(Phase2 差し替えの予行)。
    let gated = Layer1TrustPolicy::with_weight(2.5);
    assert_eq!(gated.ranking_weight(), 2.5);
    let v1 = facts_tokyo();
    let v2 = facts_tokyo();
    assert_eq!(
        gated.compute(&[v1.as_slice(), v2.as_slice()]),
        compute_layer1_trust(&[v1.as_slice(), v2.as_slice()]),
        "重みは順位寄与のみを変え、算出コア(Jaccard平均)は変えない"
    );
}

// Phase2 差し替えの予行: witness 風の前処理を足した独自 TrustPolicy でも、
// 算出コア(Jaccard平均)自体は compute_layer1_trust を再利用できる(§7 policy hook)。
struct Phase2StylePolicy;

impl TrustPolicy for Phase2StylePolicy
{
    fn name(&self) -> &str
    {
        "phase2-style(test)"
    }

    fn compute(&self, version_facts: &[&[FactTriple]]) -> Trust
    {
        // Phase2 は「witness独立性検証つき一致率」へ差し替わるが、一致率の
        // 算出コアはそのまま再利用できる、という設計不変条件の検証。
        compute_layer1_trust(version_facts)
    }

    fn ranking_weight(&self) -> f64
    {
        1.0
    }
}

#[test]
fn swapped_policy_reuses_the_same_core()
{
    let a = facts_tokyo();
    let b = facts_osaka();
    let swapped = Phase2StylePolicy;
    let base = Layer1TrustPolicy::new();
    assert_eq!(
        swapped.compute(&[a.as_slice(), b.as_slice()]),
        base.compute(&[a.as_slice(), b.as_slice()]),
        "policy を差し替えても算出コアは不変であるべき"
    );
}

// ==================================================================
// キャッシュ再導出フック(実受信コードパス)
// ==================================================================

// 同一 question_norm・任意 facts・任意 created の版を verify→insert で
// 実受信コードパスに通してキャッシュへ入れる(created を制御するため
// register(now固定)ではなく手組みコアを使う)。DummySigner(MAC)でも
// キャッシュ自身の signer で署名すれば author_sig 検証が通る。
fn ingest_version(cache: &mut SemanticCache, signer: &dyn crate::signer::Signer, q: &str, facts: Vec<FactTriple>, created: &str) -> String
{
    let core = make_core(q, facts, created, "permanent", Tier::Low);
    let env = envelope_from_core(&core, signer);
    let entry = cache.verify_envelope(&env, None).expect("検証済みエントリ構築");
    let report = cache.insert_verified(entry);
    report.entry_id
}

fn fresh_cache(tag: &str) -> (SemanticCache, std::sync::Arc<dyn crate::signer::Signer>, std::path::PathBuf)
{
    let dir = temp_dir(tag);
    let signer = new_signer(&dir, "n");
    let cache = SemanticCache::new(dir.join("store"), shared_embedder(), signer.clone(), LOCAL_THRESHOLD);
    (cache, signer, dir)
}

#[test]
fn recompute_bundle_sets_trust_and_preserves_entry_id_and_sig()
{
    // 観点6: trust 再導出は entry_id / author_sig に一切影響しない(署名境界不変)。
    let (mut cache, signer, _dir) = fresh_cache("trust_recompute");
    let q = "日本の首都はどこですか";

    // 同一 question_key・同一 facts・異なる created の2版(=別 entry_id で併存)
    let id1 = ingest_version(&mut cache, signer.as_ref(), q, facts_tokyo(), OLDER);
    let id2 = ingest_version(&mut cache, signer.as_ref(), q, facts_tokyo(), NEWER);
    assert_ne!(id1, id2, "created 違いで別版(別 entry_id)になるべき");
    assert_eq!(cache.size(), 2);

    // 再導出前の署名を記録(insert_verified 時点では trust=None)
    let sig1_before = cache.get(&id1).unwrap().author_sig.clone();
    let sig2_before = cache.get(&id2).unwrap().author_sig.clone();
    assert!(cache.get(&id1).unwrap().state.trust.is_none());

    // trust 再導出(policy 経由)
    let policy = Layer1TrustPolicy::new();
    let qkey = question_key(q);
    let computed = cache.recompute_trust_for_bundle(&qkey, &policy).expect("バンドル存在");

    // 2版一致 → agreement 1.0 / supporting 2
    assert_eq!(computed, Trust { independent_agreement: 1.0, supporting_versions: 2 });

    // 両版に同じバンドル trust が格納される
    for id in [&id1, &id2]
    {
        let e = cache.get(id).unwrap();
        assert_eq!(e.state.trust.as_ref().unwrap(), &computed);
        // entry_id は core_bytes 由来で不変、author_sig も不変(署名境界に触れない)
        assert_eq!(&e.entry_id, id);
    }
    assert_eq!(cache.get(&id1).unwrap().author_sig, sig1_before);
    assert_eq!(cache.get(&id2).unwrap().author_sig, sig2_before);
}

#[test]
fn recompute_bundle_is_locally_derived_from_version_facts()
{
    // 観点3「ローカル算出のみ」: 再導出値は、自ノードがローカル保持する版facts から
    // 算出した純粋関数の値に一致する。送信者の trust 値は EntryEnvelope に構造的に
    // 存在しない(core+署名のみ)ため、入力レベルで「送信者値不信任」が保証される。
    let (mut cache, signer, _dir) = fresh_cache("trust_local");
    let q = "日本の首都はどこですか";

    // 東京版2 + 大阪版1(矛盾混入)
    ingest_version(&mut cache, signer.as_ref(), q, facts_tokyo(), OLDER);
    ingest_version(&mut cache, signer.as_ref(), q, facts_tokyo(), NEWER);
    ingest_version(&mut cache, signer.as_ref(), q, facts_osaka(), "2026-07-15T00:00:00Z");

    let policy = Layer1TrustPolicy::new();
    let qkey = question_key(q);
    let got = cache.recompute_trust_for_bundle(&qkey, &policy).unwrap();

    // 版facts から純粋関数で独立に算出した期待値(= 1/3、supporting 3)と一致
    let expected = compute_layer1_trust(&[
        facts_tokyo().as_slice(),
        facts_tokyo().as_slice(),
        facts_osaka().as_slice(),
    ]);
    assert_eq!(got, expected);
    assert_eq!(got.supporting_versions, 3);
}

#[test]
fn recompute_all_covers_multiple_bundles()
{
    let (mut cache, signer, _dir) = fresh_cache("trust_all");
    let q_cap = "日本の首都はどこですか";
    let q_author = "Rustの作者は誰ですか";

    ingest_version(&mut cache, signer.as_ref(), q_cap, facts_tokyo(), OLDER);
    ingest_version(&mut cache, signer.as_ref(), q_cap, facts_tokyo(), NEWER);
    ingest_version(&mut cache, signer.as_ref(), q_author, vec![triple("rust", "作者", "graydon hoare")], OLDER);

    cache.recompute_trust_all(&Layer1TrustPolicy::new());

    // 2版バンドルは agreement 1.0、単独版バンドルはペア不在で 0.0
    let cap_key = question_key(q_cap);
    let author_key = question_key(q_author);
    let cap_trust = cache
        .entries()
        .iter()
        .find(|e| e.question_key == cap_key)
        .and_then(|e| e.state.trust.clone())
        .unwrap();
    let author_trust = cache
        .entries()
        .iter()
        .find(|e| e.question_key == author_key)
        .and_then(|e| e.state.trust.clone())
        .unwrap();
    assert_eq!(cap_trust, Trust { independent_agreement: 1.0, supporting_versions: 2 });
    assert_eq!(author_trust, Trust { independent_agreement: 0.0, supporting_versions: 1 });
}

#[test]
fn lookup_weighted_gate_does_not_reorder_within_bundle()
{
    // 観点4(配線): 同一 question_key の版はバンドル trust が等しいため、
    // 重み0でも重み>0でも created 主軸の順位(新しい版)が変わらない。
    // = 実測ゲートは「意思決定を動かさない助言」であることの結線確認。
    let (mut cache, signer, _dir) = fresh_cache("trust_rank");
    let q = "日本の首都はどこですか";
    let id_old = ingest_version(&mut cache, signer.as_ref(), q, facts_tokyo(), OLDER);
    let id_new = ingest_version(&mut cache, signer.as_ref(), q, facts_tokyo(), NEWER);
    cache.recompute_trust_all(&Layer1TrustPolicy::new());

    let all = |_e: &crate::entry::CacheEntry| true;
    // 重み0(既定)
    let r0 = cache.lookup_filtered_weighted(q, &all, 0.0);
    // 重み>0(ゲート有効相当)
    let r1 = cache.lookup_filtered_weighted(q, &all, 5.0);

    let hit0 = r0.entry.expect("ヒットするはず").entry_id.clone();
    let hit1 = r1.entry.expect("ヒットするはず").entry_id.clone();
    // どちらも created が新しい版を返す(重みで順位が変わらない)
    assert_eq!(hit0, id_new);
    assert_eq!(hit1, id_new);
    assert_ne!(hit0, id_old);
}

#[test]
fn trust_is_not_persisted_and_is_rederived_on_reload()
{
    // 案B: trust は導出状態なので state.json へ永続化しない(#[serde(skip)])。
    // 起動時に版集合から再導出する運用のため、永続化は冗長 I/O = 消してよい。
    // 本テストは「保存されないが、再導出すれば同じ値が復元される」ことを確認する。
    let dir = temp_dir("trust_no_persist");
    let store = dir.join("store");
    let signer = new_signer(&dir, "n");
    let q = "日本の首都はどこですか";

    // 1周目: 2版を投入し trust を再導出(メモリ上で 1.0 / 2 になる)
    {
        let mut cache =
            SemanticCache::new(store.clone(), shared_embedder(), signer.clone(), LOCAL_THRESHOLD);
        ingest_version(&mut cache, signer.as_ref(), q, facts_tokyo(), OLDER);
        ingest_version(&mut cache, signer.as_ref(), q, facts_tokyo(), NEWER);
        let t = cache.recompute_trust_for_bundle(&question_key(q), &Layer1TrustPolicy::new());
        assert_eq!(t, Some(Trust { independent_agreement: 1.0, supporting_versions: 2 }));
    }

    // 2周目: 同じ store から新しい SemanticCache をロード。
    // SemanticCache::new は load するが recompute はしない(それは NodeService::new
    // の役割)ため、trust は永続化されていない = 全エントリ None のはず。
    let mut reloaded =
        SemanticCache::new(store.clone(), shared_embedder(), signer.clone(), LOCAL_THRESHOLD);
    assert_eq!(reloaded.size(), 2, "エントリ(core+state)は永続化されロードされる");
    assert!(
        reloaded.entries().iter().all(|e| e.state.trust.is_none()),
        "trust は state.json に保存されないため、ロード直後は None であるべき(案B)"
    );

    // 再導出すれば同じ値が復元される(起動時 recompute_trust_all 相当)。
    let rederived =
        reloaded.recompute_trust_for_bundle(&question_key(q), &Layer1TrustPolicy::new());
    assert_eq!(
        rederived,
        Some(Trust { independent_agreement: 1.0, supporting_versions: 2 }),
        "永続化しなくても版集合から同じ trust を決定的に再構築できる"
    );
}

// ==================================================================
// ベンチマーク(§9-5 hot path 負荷実測)。#[ignore] 付き。
//   cargo test --release -- --ignored --nocapture bench_trust
// ==================================================================

#[test]
#[ignore]
fn bench_trust_recompute()
{
    use std::time::Instant;

    println!("\n=== S4層1 trust 再導出ベンチ(版ペア間 Jaccard 平均。O(版数^2)) ===");
    // 案B(trust 非永続化)後: recompute_trust_for_bundle は算出+メモリ更新のみ
    // (state.json 再書き込みは除去済み)。案B前は全版 save_state I/O が支配的だった。
    println!("(release 前提 / recompute_trust_for_bundle = 算出のみ・trust非永続化=案B)");

    // 版数を変えて hot path(受信/登録時の再導出)コストを計測する。
    // ペア数は k(k-1)/2 で増えるため、版数増加に対する二次的増分を観測する。
    let policy = Layer1TrustPolicy::new();
    for &k in &[2usize, 5, 10, 20, 50]
    {
        let (mut cache, signer, _dir) = fresh_cache(&format!("bench_trust_{k}"));
        let q = "共通の質問文(同一 question_key バンドル)";
        // k 版を投入(created を秒単位でずらして別 entry_id にする)
        for i in 0..k
        {
            let created = format!("2026-07-19T00:{:02}:{:02}Z", i / 60, i % 60);
            // 版ごとに facts をわずかに変え、Jaccard に分散を持たせる(全一致=無信号回避)
            let facts = if i % 3 == 0 { facts_tokyo() } else if i % 3 == 1 { facts_osaka() } else { vec![triple("日本", "首都", "京都")] };
            ingest_version(&mut cache, signer.as_ref(), q, facts, &created);
        }
        let qkey = question_key(q);

        let iters = 200usize;
        let t0 = Instant::now();
        for _ in 0..iters
        {
            let _ = cache.recompute_trust_for_bundle(&qkey, &policy);
        }
        let avg_us = t0.elapsed().as_micros() as f64 / iters as f64;
        println!("版数 k={k:>3} (ペア数 {:>5}) : recompute 平均 {avg_us:>10.2} us/回", k * (k - 1) / 2);
    }
}

#[test]
#[ignore]
fn bench_trust_compute_pure()
{
    use std::time::Instant;

    println!("\n=== S4層1 trust 算出コア単体ベンチ(compute_layer1_trust。I/Oなし) ===");
    // 純粋な算出コストだけを分離計測する(案B後は recompute もこれに漸近する)。
    for &k in &[2usize, 10, 50, 100, 200]
    {
        // k 版・各5 facts の版集合を用意
        let versions: Vec<Vec<FactTriple>> = (0..k)
            .map(|i| vec![
                triple("s", "p1", &format!("o{}", i % 4)),
                triple("s", "p2", &format!("o{}", i % 3)),
                triple("s", "p3", "共通"),
                triple("s", "p4", "共通"),
                triple("s", "p5", &format!("o{}", i % 5)),
            ])
            .collect();
        let refs: Vec<&[FactTriple]> = versions.iter().map(|v| v.as_slice()).collect();

        let iters = 500usize;
        let t0 = Instant::now();
        for _ in 0..iters
        {
            let _ = compute_layer1_trust(&refs);
        }
        let avg_us = t0.elapsed().as_micros() as f64 / iters as f64;
        println!("版数 k={k:>3} (ペア数 {:>6}) : compute 平均 {avg_us:>10.2} us/回", k * (k - 1) / 2);
    }
}
