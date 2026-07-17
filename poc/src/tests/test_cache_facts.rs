// facts(事実トリプル)を含むエントリの署名不変条件テスト(cache.rs / S2.5 形式)。
//
// S2.5(docs/S2.5_エントリ形式設計.md §1, §3)では facts は immutable_core の
// 一部として encode_core の正準バイト列に含まれ、entry_id(= sha256(core_bytes))と
// author_sig の対象になる。ここでは:
//   - facts を含むエントリでも entry_id = sha256(core_bytes) が成立する
//   - facts(core_b64 内の fact 目的語)の改ざん → ハッシュ不一致で drop(改ざん検知)
//   - 署名対象外(state.json の confidence)の改ざんは検知されない(意図的)
// を新形式へ追随して確認する。既存 test_cache.rs の core/author_sig 2経路
// (設計メモ §4)に facts 経路を足す位置づけ。register(S2 完全登録経路)を用いる。

use crate::cache::{SemanticCache, LOCAL_THRESHOLD};
use crate::embedder::MockEmbedder;
use crate::signer::DummySigner;
use crate::triples::FactTriple;
use crate::volatility::VolatilityAssessment;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

fn first_entry_file(store_dir: &Path) -> PathBuf
{
    let mut entries: Vec<PathBuf> = fs::read_dir(store_dir)
        .expect("store_dir の読み取りに失敗")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "entry").unwrap_or(false))
        .collect();
    entries.sort();
    entries.into_iter().next().expect(".entry が存在しない")
}

fn state_path_for(entry_file: &Path) -> PathBuf
{
    let id = entry_file.file_stem().unwrap().to_string_lossy().to_string();
    entry_file.with_file_name(format!("{id}.state.json"))
}

fn sha256_hex_local(data: &[u8]) -> String
{
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

// permanent 想定の評価結果(首都 = permanent 型述語)。
fn permanent_assessment() -> VolatilityAssessment
{
    VolatilityAssessment
    {
        class: "permanent".to_string(),
        confidence: 0.6,
        evidence: vec!["predicate_type:permanent".to_string()],
    }
}

fn sample_facts() -> Vec<FactTriple>
{
    vec![FactTriple { s: "日本".to_string(), p: "首都".to_string(), o: "東京".to_string() }]
}

#[test]
fn facts_entry_id_equals_sha256_of_core_bytes()
{
    // facts を含むエントリでも entry_id は core_bytes の sha256 と一致する。
    let dir = super::common::temp_dir("cache_facts_id");
    let store = dir.join("store");
    let keyfile = dir.join("node.key");

    let embedder = MockEmbedder::default();
    let signer = DummySigner::new(&keyfile).expect("signer 初期化に失敗");
    let mut cache = SemanticCache::new(store, &embedder, &signer, LOCAL_THRESHOLD);

    let facts = sample_facts();
    let e = cache.register(
        "日本の首都はどこですか",
        "日本の首都は東京です。",
        &permanent_assessment(),
        &facts,
        true,
        "全ゲート通過: 共有可",
        "mock-agent",
    );

    // entry_id = sha256(core_bytes)。
    assert_eq!(e.entry_id, sha256_hex_local(&e.core_bytes), "entry_id が core_bytes の sha256 と不一致");
    // facts は core_bytes(署名対象)にバイトとして含まれる。
    let needle = "東京".as_bytes();
    assert!(
        e.core_bytes.windows(needle.len()).any(|w| w == needle),
        "fact 目的語が core_bytes に含まれていない(署名対象化されていない)"
    );
    assert_eq!(e.core.facts, facts, "登録された facts が入力と一致しない");
}

#[test]
fn tampered_fact_dropped_by_hash_mismatch()
{
    // 検知経路(facts): 保存済み .entry の core_b64 を復号し fact 目的語("東京")の
    // 1バイトを書き換え → 再 base64 → ロードで sha256(core_bytes) != ファイル名 となり
    // ハッシュ不一致で drop される(改ざん検知)。core_bytes を触るので署名検証も
    // 失敗するが、§6 手順ではハッシュ照合(手順3)が署名検証(手順4)より先で、
    // いずれにせよ毒入りエントリは排除される。
    let dir = super::common::temp_dir("cache_facts_tamper");
    let store = dir.join("store");
    let keyfile = dir.join("node.key");

    let embedder = MockEmbedder::default();
    let signer = DummySigner::new(&keyfile).expect("signer 初期化に失敗");

    {
        let mut cache = SemanticCache::new(store.clone(), &embedder, &signer, LOCAL_THRESHOLD);
        cache.register(
            "日本の首都はどこですか",
            "日本の首都は東京です。",
            &permanent_assessment(),
            &sample_facts(),
            true,
            "共有可",
            "mock-agent",
        );
        assert_eq!(cache.size(), 1);
    }

    let path = first_entry_file(&store);
    let data = fs::read_to_string(&path).expect("エントリ読込に失敗");
    let mut j: serde_json::Value = serde_json::from_str(&data).expect("JSON パースに失敗");
    let core_b64 = j["core_b64"].as_str().expect("core_b64 が無い").to_string();
    let mut core_bytes = B64.decode(&core_b64).expect("core_b64 復号に失敗");
    // fact 目的語 "東京" の先頭バイトを反転する(answer ではなく facts を狙う)。
    let needle = "東京".as_bytes();
    let pos = core_bytes
        .windows(needle.len())
        .position(|w| w == needle)
        .expect("fact 目的語のバイト列が core に見つからない");
    core_bytes[pos] ^= 0xFF;
    j["core_b64"] = serde_json::Value::String(B64.encode(&core_bytes));
    fs::write(&path, serde_json::to_string_pretty(&j).unwrap()).expect("書き戻しに失敗");

    let reloaded = SemanticCache::new(store, &embedder, &signer, LOCAL_THRESHOLD);
    assert_eq!(reloaded.size(), 0, "facts 改ざんが検知されず残ってしまった");
}

#[test]
fn tampered_state_confidence_is_not_signed()
{
    // 署名対象外(state.json の volatility_confidence)の書き換えは検知されない(意図的)。
    // §2/§6: 確信度は再評価で更新される可変推定値であり core(署名対象)の外にある。
    // ここでは「その設計どおり confidence 改ざんではエントリが落ちない」ことを確認し、
    // facts/core 改ざん(検知される)との非対称性を明示する。
    let dir = super::common::temp_dir("cache_conf_unsigned");
    let store = dir.join("store");
    let keyfile = dir.join("node.key");

    let embedder = MockEmbedder::default();
    let signer = DummySigner::new(&keyfile).expect("signer 初期化に失敗");

    {
        let mut cache = SemanticCache::new(store.clone(), &embedder, &signer, LOCAL_THRESHOLD);
        cache.register(
            "日本の首都はどこですか",
            "日本の首都は東京です。",
            &permanent_assessment(),
            &sample_facts(),
            true,
            "共有可",
            "mock-agent",
        );
    }

    let entry_file = first_entry_file(&store);
    let spath = state_path_for(&entry_file);
    let sdata = fs::read_to_string(&spath).expect("state.json 読込に失敗");
    let mut s: serde_json::Value = serde_json::from_str(&sdata).expect("state JSON パース失敗");
    s["volatility_confidence"] = serde_json::json!(0.01); // 署名対象外なので検知されないはず
    fs::write(&spath, serde_json::to_string_pretty(&s).unwrap()).expect("state 書き戻し失敗");

    let reloaded = SemanticCache::new(store, &embedder, &signer, LOCAL_THRESHOLD);
    assert_eq!(reloaded.size(), 1, "署名対象外の confidence 改ざんでエントリが落ちた(設計と不整合)");
}
