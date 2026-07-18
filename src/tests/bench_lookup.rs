// 全複製ローカル検索(コサイン総当たり)のスケール計測(S3設計ノート §5)。
//
// 実行方法:
//   cargo test --release -- --ignored --nocapture bench_lookup
//
// 注意:
//   - #[ignore] 付きなので通常の `cargo test` ではスキップされる。
//   - 必ず --release で計測すること(debug だと内積が最適化されず非現実的に遅い)。
//   - --nocapture がないと println! の計測結果が出ない。
//
// 設計ノート §5 の想定「〜10^5 でも hot path 許容(n=10,000 で約5.4ms、
// 10^5 で数十ms級)」を n=100/1,000/10,000 で裏取りする。

use super::common::temp_dir;
use crate::cache::{SemanticCache, LOCAL_THRESHOLD};
use crate::embedder::MockEmbedder;
use crate::node::Crl;
use crate::policy::{CertPolicy, CompanyCertPolicy};
use crate::signer::create_signer;
use std::sync::Arc;
use std::time::Instant;

#[test]
#[ignore]
fn bench_lookup()
{
    let sizes = [100usize, 1_000, 10_000];
    let iters = 100usize;

    println!("\n=== SemanticCache.lookup() ベンチマーク(全複製ローカル検索) ===");
    println!("(release ビルド前提 / iters={iters} 回平均 / MockEmbedder dim=512)");

    for &n in &sizes
    {
        let dir = temp_dir(&format!("bench_{n}"));
        let embedder = Arc::new(MockEmbedder::default());
        let signer = Arc::from(create_signer(&dir.join("node.key")).expect("signer 初期化に失敗"));
        let mut cache = SemanticCache::new(dir.join("store"), embedder, signer, LOCAL_THRESHOLD);

        // n 件のシンセティックエントリ(質問文をループ変数で一意化)。
        for i in 0..n
        {
            let q = format!("synthetic question number {i} about topic {i}");
            let a = format!("synthetic answer body for entry {i}");
            cache.register_entry(&q, &a, "slow", true, "共有可", "mock-agent");
        }
        assert_eq!(cache.size(), n);

        // 登録済み質問群を巡回して検索(ヒット経路を踏む=総当たりの最悪計算量)。
        let t0 = Instant::now();
        for k in 0..iters
        {
            let idx = k % n;
            let q = format!("synthetic question number {idx} about topic {idx}");
            let _ = cache.lookup(&q);
        }
        let elapsed = t0.elapsed();
        let avg_us = elapsed.as_micros() as f64 / iters as f64;
        println!("n={n:>6} 件 : lookup 平均 {avg_us:>10.2} us/回");
    }
}

// H-1 失効フィルタ(is_author_revoked)配線後の検索経路レイテンシ計測
// (脅威レビュー H-1)。実運用の検索経路 sync::NodeService::is_searchable は
// lookup_filtered へ「著者が CRL 失効していないか」を都度照合する closure を渡す。
// is_author_revoked は author_pub から node_id(sha256)を毎エントリ再計算するため、
// 全複製ブルートフォース検索に上乗せされる。その上乗せが「顕著な劣化なし」で
// あることを、素の lookup と同一データ・同一巡回で対照計測する。
//
// 実行方法:
//   cargo test --release -- --ignored --nocapture bench_lookup_revocation_filter
#[test]
#[ignore]
fn bench_lookup_revocation_filter()
{
    let sizes = [100usize, 1_000, 10_000];
    let iters = 100usize;

    println!("\n=== lookup vs lookup_filtered(H-1 失効フィルタ配線)ベンチマーク ===");
    println!("(release ビルド前提 / iters={iters} 回平均 / CRL は空 = 全エントリ通過)");

    for &n in &sizes
    {
        let dir = temp_dir(&format!("bench_revfilter_{n}"));
        let embedder = Arc::new(MockEmbedder::default());
        let signer = Arc::from(create_signer(&dir.join("node.key")).expect("signer 初期化に失敗"));
        // 失効フィルタ用ポリシー(CRL は空 = is_author_revoked は常に false だが、
        // node_id 再計算 + CRL 走査のコストは実運用どおり毎エントリ発生する)。
        let ca = Arc::from(create_signer(&dir.join("ca.key")).expect("CA signer 初期化に失敗"));
        let pol = Arc::new(CompanyCertPolicy::new(ca, ""));
        pol.set_crl(Crl::default()); // 空CRLを明示(実運用の set_crl 経路を踏む)
        let pol: Arc<dyn CertPolicy> = pol;
        let mut cache = SemanticCache::new(dir.join("store"), embedder, signer, LOCAL_THRESHOLD);

        for i in 0..n
        {
            let q = format!("synthetic question number {i} about topic {i}");
            let a = format!("synthetic answer body for entry {i}");
            cache.register_entry(&q, &a, "slow", true, "共有可", "mock-agent");
        }
        assert_eq!(cache.size(), n);

        // 素の lookup。
        let t0 = Instant::now();
        for k in 0..iters
        {
            let idx = k % n;
            let q = format!("synthetic question number {idx} about topic {idx}");
            let _ = cache.lookup(&q);
        }
        let plain_us = t0.elapsed().as_micros() as f64 / iters as f64;

        // 失効フィルタ配線(実運用の is_searchable の失効照合部分と同等)。
        let t1 = Instant::now();
        for k in 0..iters
        {
            let idx = k % n;
            let q = format!("synthetic question number {idx} about topic {idx}");
            let _ = cache.lookup_filtered(&q, &|e| !pol.is_author_revoked(&e.author_pub));
        }
        let filtered_us = t1.elapsed().as_micros() as f64 / iters as f64;

        let overhead = if plain_us > 0.0 { (filtered_us - plain_us) / plain_us * 100.0 } else { 0.0 };
        println!(
            "n={n:>6} 件 : lookup {plain_us:>10.2} us/回 / filtered {filtered_us:>10.2} us/回 (上乗せ {overhead:+.1}%)"
        );
    }
}
