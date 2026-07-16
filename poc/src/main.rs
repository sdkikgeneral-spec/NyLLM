// Winny型 Semantic Cache PoC — 最小ループの通しデモ。
//
//   質問 → Embedding → 意味検索
//     ├ ヒット(類似度 >= しきい値) → キャッシュ回答
//     └ ミス → Agent推論 → 揮発性タグ → 共有可否ゲート → 署名付き登録 → 回答
//
// 使い方:
//   semantic_cache_poc            … キャッシュを初期化してデモ実行
//   semantic_cache_poc --keep     … 既存キャッシュを保持したまま実行
mod agent;
mod cache;
mod embedder;
mod signer;
mod volatility;

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
        let vol = volatility::classify_volatility(q);
        let dec = volatility::share_gate(q, &vol);
        let e = cache.register_entry(q, &answer, &vol, dec.shareable, &dec.reason, agent.name());
        println!("     A: {}", clip_utf8(&answer, 80));
        println!(
            "     volatility={} / 共有={} ({})",
            vol,
            if dec.shareable { "可" } else { "不可" },
            dec.reason
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
