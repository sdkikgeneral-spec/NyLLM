// SemanticCache.lookup() の簡易ベンチマーク(cache.rs)。
//
// 実行方法:
//   cargo test --release -- --ignored --nocapture bench_lookup
//
// 注意:
//   - #[ignore] 付きなので通常の `cargo test` ではスキップされる。
//   - 必ず --release で実行すること。debug ビルドだと総当たり内積が
//     最適化されず、非現実的に遅い数値になる。
//   - --nocapture を付けないと println! の計測結果が表示されない。

use crate::cache::{SemanticCache, LOCAL_THRESHOLD};
use crate::embedder::MockEmbedder;
use crate::signer::DummySigner;
use std::time::Instant;

#[test]
#[ignore]
fn bench_lookup()
{
    // 計測対象の件数。件数を変えて O(n) 総当たりのスケールを見る。
    let sizes = [100usize, 1_000, 10_000];
    // 各件数での lookup 呼び出し回数(平均を取るため)。
    let iters = 100usize;

    println!("\n=== SemanticCache.lookup() ベンチマーク ===");
    println!("(release ビルド前提 / iters={iters} 回の平均)");

    for &n in &sizes
    {
        let dir = super::common::temp_dir(&format!("bench_{n}"));
        let store = dir.join("store");
        let keyfile = dir.join("node.key");

        // embedder / signer は cache より長生きさせる。
        let embedder = MockEmbedder::default();
        let signer = DummySigner::new(&keyfile).expect("signer 初期化に失敗");
        let mut cache = SemanticCache::new(store, &embedder, &signer, LOCAL_THRESHOLD);

        // n 件のシンセティックエントリを登録(質問文はループ変数で一意化)。
        for i in 0..n
        {
            let q = format!("synthetic question number {i} about topic {i}");
            let a = format!("synthetic answer body for entry {i}");
            cache.register_entry(&q, &a, "slow", true, "共有可", "mock-agent");
        }
        assert_eq!(cache.size(), n);

        // 検索対象は登録済みの質問群から巡回して選ぶ(ヒット経路も踏む)。
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
