// facts(事実トリプル)を含むエントリの署名不変条件テスト(cache.rs)。
//
// S2 で CacheEntry に facts: Vec<FactTriple> が追加され、signed_payload() の
// 署名対象に含まれた(question+answer+created+volatility+facts)。ここでは:
//   - facts を含むエントリでも entry_id = sha256(signed_payload) が成立する
//   - facts の改ざん → 再計算ハッシュ不一致で除外(改ざん"検知")
// を確認する。既存 test_cache.rs の answer/author_sig 改ざん2経路(設計メモ §4)に
// facts 経路を追加する位置づけ。register_judged_entry(S2登録経路)を用いる。
//
// 注意: volatility_confidence / volatility_evidence は §10 の再評価で更新される
// 可変推定値のため署名対象外。これらの改ざんは検知されない(意図的)ことも確認する。

use crate::cache::{SemanticCache, LOCAL_THRESHOLD};
use crate::embedder::MockEmbedder;
use crate::signer::DummySigner;
use crate::triples::FactTriple;
use crate::volatility::VolatilityAssessment;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

fn first_entry_json(store_dir: &Path) -> PathBuf
{
    let mut jsons: Vec<PathBuf> = fs::read_dir(store_dir)
        .expect("store_dir の読み取りに失敗")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
        .collect();
    jsons.sort();
    jsons.into_iter().next().expect("エントリ .json が存在しない")
}

fn sha256_hex_local(data: &[u8]) -> String
{
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

// テスト用: permanent 想定の facts 付き評価結果を組み立てる。
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
fn judged_entry_id_equals_sha256_including_facts()
{
    // facts を含むエントリでも entry_id は signed_payload() の sha256 と一致する。
    let dir = super::common::temp_dir("cache_facts_id");
    let store = dir.join("store");
    let keyfile = dir.join("node.key");

    let embedder = MockEmbedder::default();
    let signer = DummySigner::new(&keyfile).expect("signer 初期化に失敗");
    let mut cache = SemanticCache::new(store, &embedder, &signer, LOCAL_THRESHOLD);

    let assessment = permanent_assessment();
    let facts = sample_facts();
    let e = cache.register_judged_entry(
        "日本の首都はどこですか",
        "日本の首都は東京です。",
        &assessment,
        &facts,
        true,
        "全ゲート通過: 共有可",
        "mock-agent",
    );

    // facts がペイロードに含まれていること(署名対象)を間接確認:
    // signed_payload 文字列に述語・目的語が現れる。
    let payload = e.signed_payload();
    assert!(payload.contains("首都") && payload.contains("東京"), "facts が署名対象に含まれていない");
    let expected = sha256_hex_local(payload.as_bytes());
    assert_eq!(e.entry_id, expected, "facts 込みで entry_id が sha256(signed_payload) と一致しない");
    assert_eq!(e.facts, facts, "登録された facts が入力と一致しない");
}

#[test]
fn tampered_fact_dropped_by_hash_mismatch()
{
    // 検知経路3(facts): 保存済み JSON の facts[0].o を書き換えると、
    // facts は signed_payload に含まれるため再計算ハッシュがずれ、
    // entry_id と一致しなくなる → ハッシュ不一致で除外(改ざん"検知")。
    let dir = super::common::temp_dir("cache_facts_tamper");
    let store = dir.join("store");
    let keyfile = dir.join("node.key");

    let embedder = MockEmbedder::default();
    let signer = DummySigner::new(&keyfile).expect("signer 初期化に失敗");

    {
        let mut cache = SemanticCache::new(store.clone(), &embedder, &signer, LOCAL_THRESHOLD);
        cache.register_judged_entry(
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

    // facts[0].o("東京")だけを毒入り値に書き換える(answer は触らない)。
    let path = first_entry_json(&store);
    let data = fs::read_to_string(&path).expect("エントリ読込に失敗");
    let mut j: serde_json::Value = serde_json::from_str(&data).expect("JSON パースに失敗");
    j["facts"][0]["o"] = serde_json::Value::String("大阪(毒入り)".to_string());
    fs::write(&path, serde_json::to_string_pretty(&j).unwrap()).expect("書き戻しに失敗");

    let reloaded = SemanticCache::new(store, &embedder, &signer, LOCAL_THRESHOLD);
    assert_eq!(reloaded.size(), 0, "facts 改ざんが検知されず残ってしまった");
}

#[test]
fn tampered_volatility_confidence_is_not_signed()
{
    // 署名対象外フィールド(volatility_confidence)の書き換えは検知されない(意図的)。
    // §6/§10: 揮発性の確信度は再評価で更新される可変推定値のため署名対象に含めない。
    // ここでは「その設計どおり confidence 改ざんではエントリが落ちない」ことを確認する
    // (facts/answer/volatility クラスの改ざんとの非対称性を明示)。
    let dir = super::common::temp_dir("cache_conf_unsigned");
    let store = dir.join("store");
    let keyfile = dir.join("node.key");

    let embedder = MockEmbedder::default();
    let signer = DummySigner::new(&keyfile).expect("signer 初期化に失敗");

    {
        let mut cache = SemanticCache::new(store.clone(), &embedder, &signer, LOCAL_THRESHOLD);
        cache.register_judged_entry(
            "日本の首都はどこですか",
            "日本の首都は東京です。",
            &permanent_assessment(),
            &sample_facts(),
            true,
            "共有可",
            "mock-agent",
        );
    }

    let path = first_entry_json(&store);
    let data = fs::read_to_string(&path).expect("エントリ読込に失敗");
    let mut j: serde_json::Value = serde_json::from_str(&data).expect("JSON パースに失敗");
    // 確信度を書き換える(署名対象外なので検知されないはず)。
    j["volatility_confidence"] = serde_json::json!(0.01);
    fs::write(&path, serde_json::to_string_pretty(&j).unwrap()).expect("書き戻しに失敗");

    let reloaded = SemanticCache::new(store, &embedder, &signer, LOCAL_THRESHOLD);
    assert_eq!(
        reloaded.size(),
        1,
        "署名対象外の confidence 改ざんでエントリが落ちた(設計と不整合)"
    );
}
