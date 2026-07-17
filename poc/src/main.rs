// Winny型 Semantic Cache PoC — 最小ループの通しデモ。
//
//   質問 → Embedding → 意味検索
//     ├ ヒット(類似度 >= しきい値) → キャッシュ回答
//     └ ミス → Agent推論 → 判定パイプライン(Architecture §7)
//               [L0語彙 → L2自己申告 → 案4トリプル分解 → §10.1揮発性確定]
//               → 署名付き登録 → 回答
//
// 使い方:
//   semantic_cache_poc            … キャッシュを初期化してデモ実行
//   semantic_cache_poc --keep     … 既存キャッシュを保持したまま実行
mod agent;
mod cache;
mod embedder;
mod pipeline;
mod signer;
mod triples;
mod volatility;

// テストモジュールの配線。
// inline な mod tests の子モジュールは src/tests/ を基準に解決されるため、
// #[path] はディレクトリを二重に付けず tests/ 内のファイル名だけを指定する。
#[cfg(test)]
mod tests
{
    #[path = "common.rs"]
    mod common;
    #[path = "test_cache.rs"]
    mod test_cache;
    #[path = "test_cache_facts.rs"]
    mod test_cache_facts;
    #[path = "test_volatility.rs"]
    mod test_volatility;
    #[path = "test_finalize_volatility.rs"]
    mod test_finalize_volatility;
    #[path = "test_triples.rs"]
    mod test_triples;
    #[path = "test_pipeline.rs"]
    mod test_pipeline;
    #[path = "test_signer.rs"]
    mod test_signer;
    #[path = "bench_cache.rs"]
    mod bench_cache;
    #[path = "bench_pipeline.rs"]
    mod bench_pipeline;
}

use cache::{SemanticCache, LOCAL_THRESHOLD, SHARED_THRESHOLD};
use std::env;
use std::fs;
use std::path::Path;
use std::time::Instant;

// UTF-8境界を壊さないよう max_bytes 以内で切り詰める
fn clip_utf8(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut n = max_bytes;
    while n > 0 && !s.is_char_boundary(n) {
        n -= 1;
    }
    format!("{}...", &s[..n])
}

fn main() {
    let keep = env::args().skip(1).any(|a| a == "--keep");

    let store = Path::new("cache_store");
    let keyfile = Path::new("keys").join("node.key");
    if !keep && store.exists() {
        fs::remove_dir_all(store).expect("既存cache_storeの削除に失敗");
    }

    let embedder = embedder::create_embedder();
    let signer = signer::create_signer(&keyfile).expect("signerの初期化に失敗");
    let agent = agent::create_agent();
    let mut cache = SemanticCache::new(
        store.to_path_buf(),
        embedder.as_ref(),
        signer.as_ref(),
        LOCAL_THRESHOLD,
    );

    println!("========================================================");
    println!("embedder : {} (dim={})", embedder.name(), embedder.dim());
    println!("signer   : {}", signer.name());
    println!("agent    : {}", agent.name());
    println!(
        "threshold: {:.2} (共有想定なら {:.2}+)",
        cache.threshold(),
        SHARED_THRESHOLD
    );
    let pk = signer.public_key_hex();
    println!("node pub : {}...", &pk[..16.min(pk.len())]);
    println!("既存キャッシュ: {} 件", cache.size());
    println!("========================================================");

    let questions = [
        "Winnyとは何ですか?",
        "Winnyとは何ですか?",             // 完全一致 → ヒット(sim=1.0)
        "Winnyって何?",                   // 言い換え → Mock埋め込みでは類似度を表示
        "P2Pの仕組みを教えてください",
        "日本の首都はどこですか?",         // 案4所有格分解 → permanent型述語 → permanent
        "最新のClaudeのモデルは何ですか?", // volatile → 共有不可
        "おすすめのエディタはどれですか?", // 主観 → 共有不可
    ];

    let mut hits = 0;
    let mut misses = 0;
    for q in questions {
        println!("\nQ: {q}");
        let t0 = Instant::now();
        let r = cache.lookup(q);
        let us = t0.elapsed().as_micros();
        if let Some(entry) = r.entry {
            hits += 1;
            println!(
                "  -> HIT  (sim={:.3}, {us} us) 元の質問: \"{}\"",
                r.similarity, entry.question
            );
            println!("     A: {}", clip_utf8(&entry.answer, 80));
            continue;
        }
        misses += 1;
        println!(
            "  -> MISS (best sim={:.3}) → Agent({}) へ推論委譲",
            r.similarity,
            agent.name()
        );
        let answer = agent.ask(q);
        // 判定パイプライン(Architecture §7): L0 → L2 → 案4 → §10.1確定 を1本で通す
        let report = pipeline::judge_entry(q, &answer, agent.as_ref());
        let e = cache.register_judged_entry(
            q,
            &answer,
            &report.volatility,
            &report.decomposition.triples,
            report.shareable,
            &report.share_reason,
            agent.name(),
        );
        println!("     A: {}", clip_utf8(&answer, 80));
        println!(
            "     [L0]   volatility={} / {}",
            report.l0_volatility,
            if report.l0_gate.shareable {
                "語彙ゲート通過".to_string()
            } else {
                format!("ブロック: {}", report.l0_gate.reason)
            }
        );
        println!(
            "     [L2]   自己申告: 単独回答可={} 事実型={} 申告volatility={}",
            report.declaration.context_independent,
            report.declaration.factual,
            report.declaration.volatility
        );
        println!(
            "     [案4]  トリプル分解: {}件 ({}){}",
            report.decomposition.triples.len(),
            if report.decomposition.success { "成功" } else { "失敗" },
            report
                .decomposition
                .triples
                .first()
                .map(|t| format!(" 例: ({}, {}, {})", t.s, t.p, clip_utf8(&t.o, 40)))
                .unwrap_or_default()
        );
        println!(
            "     [確定] volatility={} (conf={:.2}) 根拠=[{}]",
            report.volatility.class,
            report.volatility.confidence,
            report.volatility.evidence.join(", ")
        );
        println!(
            "     共有={} ({}){}",
            if report.shareable { "可" } else { "不可" },
            report.share_reason,
            report
                .blocked_at
                .map(|s| format!(" blocked_at={s:?}"))
                .unwrap_or_default()
        );
        println!(
            "     登録 id={}... sig={}...",
            &e.entry_id[..16.min(e.entry_id.len())],
            &e.author_sig[..16.min(e.author_sig.len())]
        );
    }

    println!("\n========================================================");
    println!(
        "結果: {hits} hits / {misses} misses / キャッシュ {} 件",
        cache.size()
    );

    // 改ざん検知デモ: 保存済みエントリのanswerを書き換えて再読込
    println!("\n--- 改ざん検知デモ ---");
    if let Ok(rd) = fs::read_dir(store) {
        for entry in rd.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                let data = fs::read_to_string(&path).expect("改ざんデモ: エントリ読込に失敗");
                let mut j: serde_json::Value =
                    serde_json::from_str(&data).expect("改ざんデモ: JSONパースに失敗");
                if let Some(answer) = j.get("answer").and_then(|a| a.as_str()) {
                    let tampered = format!("【毒入り】{answer}");
                    j["answer"] = serde_json::Value::String(tampered);
                }
                fs::write(&path, serde_json::to_string_pretty(&j).unwrap())
                    .expect("改ざんデモ: エントリ書き込みに失敗");
                break; // 1件だけ改ざん
            }
        }
    }
    let reloaded = SemanticCache::new(
        store.to_path_buf(),
        embedder.as_ref(),
        signer.as_ref(),
        LOCAL_THRESHOLD,
    );
    println!(
        "1件を書き換え → 再読込後の有効エントリ: {} 件 (改ざん分は除外)",
        reloaded.size()
    );
}
