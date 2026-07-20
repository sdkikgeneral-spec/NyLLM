// S5 §3(d) ソース側 revocation フィルタ(Digest 列挙経路)のスケール計測。
//
// 当セッション実装(2026-07-20)のうち、唯一 O(n) スケールに関わるのが
// sync::NodeService::handle_digest_request のフィルタ述語である:
//   cache.entries().iter().filter(|e|
//       e.state.shareable
//       && !cert.is_author_revoked(&e.author_pub)   // H-1(既存)
//       && !revocation.is_revoked(&e.entry_id))     // §3(d)(本セッション追加)
// 本ベンチは §3(d) が追加した per-entry 述語 `revocation.is_revoked` の上乗せ
// コストを、全エントリ列挙に対して直接計測する。既存の
// bench_lookup_revocation_filter が検索経路で H-1(is_author_revoked=毎エントリ
// sha256)を測るのと同じ「フィルタ述語を全複製に適用」方式を、Digest 経路の
// エントリ単位失効へ適用したもの。
//
// 対照:
//   - baseline = 実コードの NoRevocationPolicy(Phase1 本番の既定。is_revoked は
//     常に false を返す自明実装 → 上乗せ ≒ 0 が期待値)。
//   - stub     = Phase2 代表実装(HashSet による entry_id 照合。O(1)/エントリ)。
// これにより「Phase1 では実質ゼロ・Phase2 で実 revocation を積んでも O(1)/エントリ」
// を数値で裏取りする(スペック §4.1 OQ#5「専用ベンチ不要=is_author_revoked と
// 同格」の主張を、より安価な述語で追認する)。
//
// なお共有キルスイッチ(sharing_active())は handle_digest_request 呼び出しあたり
// 1回の AtomicBool load(per-entry ではない)であり、オーダーに影響しないため
// 本ベンチの対象外(機能面は test_sharing_killswitch.rs が担保)。
//
// 実行方法:
//   cargo test --release -- --ignored --nocapture bench_digest_source_side

use super::common::temp_dir;
use crate::cache::{SemanticCache, LOCAL_THRESHOLD};
use crate::embedder::MockEmbedder;
use crate::policy::{NoRevocationPolicy, RevocationPolicy};
use crate::signer::create_signer;
use std::collections::HashSet;
use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

// Phase2 代表の失効ポリシー(HashSet による entry_id 照合)。
struct StubRevocation
{
    revoked: HashSet<String>,
}

impl RevocationPolicy for StubRevocation
{
    fn name(&self) -> &str
    {
        "stub-bench"
    }

    fn is_revoked(&self, entry_id: &str) -> bool
    {
        self.revoked.contains(entry_id)
    }
}

#[test]
#[ignore]
fn bench_digest_source_side_revocation_filter()
{
    let sizes = [100usize, 1_000, 10_000];
    let iters = 100usize;

    println!("\n=== §3(d) Digest 列挙フィルタ(revocation.is_revoked)ベンチ ===");
    println!(
        "(release 前提 / iters={iters} 回平均 / baseline=NoRevocationPolicy〔Phase1本番〕/ \
         stub=HashSet照合〔Phase2代表・約1%失効〕)"
    );

    for &n in &sizes
    {
        let dir = temp_dir(&format!("bench_digest_{n}"));
        let embedder = Arc::new(MockEmbedder::default());
        let signer = Arc::from(create_signer(&dir.join("node.key")).expect("signer 初期化に失敗"));
        let mut cache = SemanticCache::new(dir.join("store"), embedder, signer, LOCAL_THRESHOLD);

        for i in 0..n
        {
            let q = format!("synthetic question number {i} about topic {i}");
            let a = format!("synthetic answer body for entry {i}");
            // shareable=true で登録(Digest 列挙対象になる = handle_digest_request の
            // フィルタ第一項 e.state.shareable を通す)。
            cache.register_entry(&q, &a, "slow", true, "共有可", "mock-agent");
        }
        assert_eq!(cache.size(), n);

        // stub 用の失効集合(全体の約1%を失効扱いにする。ヒット有無に依らず
        // HashSet 照合コスト自体が per-entry で発生する)。
        let mut set = HashSet::new();
        for (i, e) in cache.entries().iter().enumerate()
        {
            if i % 100 == 0
            {
                set.insert(e.entry_id.clone());
            }
        }
        let no_rev = NoRevocationPolicy;
        let stub = StubRevocation { revoked: set };

        // baseline: 実コードの NoRevocationPolicy 述語(Phase1 本番の既定経路)。
        let t0 = Instant::now();
        for _ in 0..iters
        {
            let c = cache
                .entries()
                .iter()
                .filter(|e| e.state.shareable && !no_rev.is_revoked(&e.entry_id))
                .count();
            black_box(c);
        }
        let base_us = t0.elapsed().as_micros() as f64 / iters as f64;

        // stub: Phase2 代表(HashSet 照合)述語。
        let t1 = Instant::now();
        for _ in 0..iters
        {
            let c = cache
                .entries()
                .iter()
                .filter(|e| e.state.shareable && !stub.is_revoked(&e.entry_id))
                .count();
            black_box(c);
        }
        let stub_us = t1.elapsed().as_micros() as f64 / iters as f64;

        let overhead = if base_us > 0.0 { (stub_us - base_us) / base_us * 100.0 } else { 0.0 };
        println!(
            "n={n:>6} 件 : baseline(NoRev) {base_us:>8.2} us/回 / stub(HashSet) {stub_us:>8.2} us/回 (上乗せ {overhead:+.1}%)"
        );
    }
}
