// SemanticCache のテスト(cache.rs)。
//
// 検証対象:
//   - entry_id が signed_payload の sha256(hex) と一致する(内容ハッシュの正当性)
//   - 正常エントリは再ロードでも生き残る(ポジティブコントロール)
//   - answer 改ざん → ハッシュ不一致で除外(改ざん"検知")     … 設計メモ §4
//   - author_sig 改ざん → 署名検証失敗で除外(詐称"防止")     … 設計メモ §4
//   - HIT / MISS のしきい値挙動
//
// 注意: SemanticCache は embedder / signer を参照で保持するため、
//       これらは cache より先に束縛して長生きさせる。

use crate::cache::{SemanticCache, LOCAL_THRESHOLD};
use crate::embedder::MockEmbedder;
use crate::signer::DummySigner;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

// store_dir 内の最初の .json エントリのパスを返す。
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

// テスト内で独立に sha256(hex) を計算する(cache.rs 内部関数には依存しない)。
fn sha256_hex_local(data: &[u8]) -> String
{
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

#[test]
fn entry_id_equals_sha256_of_signed_payload()
{
    let dir = super::common::temp_dir("cache_id");
    let store = dir.join("store");
    let keyfile = dir.join("node.key");

    let embedder = MockEmbedder::default();
    let signer = DummySigner::new(&keyfile).expect("signer 初期化に失敗");
    let mut cache = SemanticCache::new(store, &embedder, &signer, LOCAL_THRESHOLD);

    let e = cache.register_entry(
        "水の沸点は摂氏何度ですか",
        "1気圧では摂氏100度です",
        "slow",
        true,
        "文脈自立 かつ 非volatile: 共有可",
        "mock-agent",
    );

    // entry_id は signed_payload() の sha256(hex) と一致しなければならない。
    // signed_payload() は pub API。ハッシュ計算はテスト側で独立に再現する。
    let expected = sha256_hex_local(e.signed_payload().as_bytes());
    assert_eq!(e.entry_id, expected, "entry_id が signed_payload の sha256 と一致しない");
}

#[test]
fn valid_entry_survives_reload()
{
    // ポジティブコントロール: 改ざんしなければ再ロードでも生き残る。
    let dir = super::common::temp_dir("cache_reload_ok");
    let store = dir.join("store");
    let keyfile = dir.join("node.key");

    let embedder = MockEmbedder::default();
    let signer = DummySigner::new(&keyfile).expect("signer 初期化に失敗");

    {
        let mut cache = SemanticCache::new(store.clone(), &embedder, &signer, LOCAL_THRESHOLD);
        cache.register_entry(
            "水の沸点は摂氏何度ですか",
            "1気圧では摂氏100度です",
            "slow",
            true,
            "共有可",
            "mock-agent",
        );
        assert_eq!(cache.size(), 1);
    }

    // 同じ store_dir / embedder / signer で作り直しても件数は不変。
    let reloaded = SemanticCache::new(store, &embedder, &signer, LOCAL_THRESHOLD);
    assert_eq!(reloaded.size(), 1, "正常エントリが再ロードで失われた");
}

#[test]
fn tampered_answer_dropped_by_hash_mismatch()
{
    // 検知経路1: 署名対象(answer)の改ざん。
    //   answer は signed_payload に含まれるため、書き換えると再計算ハッシュが
    //   ずれ、entry_id と一致しなくなる → ハッシュ不一致で除外(改ざん"検知")。
    //   これは下の author_sig テスト(詐称防止)とは別の検知経路(設計メモ §4)。
    let dir = super::common::temp_dir("cache_tamper_hash");
    let store = dir.join("store");
    let keyfile = dir.join("node.key");

    let embedder = MockEmbedder::default();
    let signer = DummySigner::new(&keyfile).expect("signer 初期化に失敗");

    {
        let mut cache = SemanticCache::new(store.clone(), &embedder, &signer, LOCAL_THRESHOLD);
        cache.register_entry(
            "水の沸点は摂氏何度ですか",
            "1気圧では摂氏100度です",
            "slow",
            true,
            "共有可",
            "mock-agent",
        );
        assert_eq!(cache.size(), 1);
    }

    // 保存済み JSON の answer だけを書き換える(main.rs の改ざんデモと同手法)。
    let path = first_entry_json(&store);
    let data = fs::read_to_string(&path).expect("エントリ読込に失敗");
    let mut j: serde_json::Value = serde_json::from_str(&data).expect("JSON パースに失敗");
    j["answer"] = serde_json::Value::String("【毒入り】改ざんされた回答".to_string());
    fs::write(&path, serde_json::to_string_pretty(&j).unwrap()).expect("書き戻しに失敗");

    let reloaded = SemanticCache::new(store, &embedder, &signer, LOCAL_THRESHOLD);
    assert_eq!(reloaded.size(), 0, "answer 改ざんが検知されず残ってしまった");
}

#[test]
fn tampered_signature_dropped_by_verify_failure()
{
    // 検知経路2: 署名(author_sig)のみの改ざん。
    //   author_sig は signed_payload に含まれない(cache.rs のスキーマ参照)ため、
    //   書き換えても内容ハッシュ(entry_id)は一致したままになる。
    //   しかし署名検証(MAC 再計算)が失敗するため除外される = 詐称"防止"。
    //   上の answer テスト(改ざん検知)とは独立した経路(設計メモ §4)。
    let dir = super::common::temp_dir("cache_tamper_sig");
    let store = dir.join("store");
    let keyfile = dir.join("node.key");

    let embedder = MockEmbedder::default();
    let signer = DummySigner::new(&keyfile).expect("signer 初期化に失敗");

    let orig_sig;
    {
        let mut cache = SemanticCache::new(store.clone(), &embedder, &signer, LOCAL_THRESHOLD);
        let e = cache.register_entry(
            "水の沸点は摂氏何度ですか",
            "1気圧では摂氏100度です",
            "slow",
            true,
            "共有可",
            "mock-agent",
        );
        orig_sig = e.author_sig.clone();
        assert_eq!(cache.size(), 1);
    }

    // author_sig だけを別の値に差し替える(内容ハッシュは変えない)。
    let path = first_entry_json(&store);
    let data = fs::read_to_string(&path).expect("エントリ読込に失敗");
    let mut j: serde_json::Value = serde_json::from_str(&data).expect("JSON パースに失敗");
    let mut forged: String = orig_sig.clone();
    // 先頭文字を別の16進文字に変えて必ず異なる署名文字列にする。
    let replacement = if forged.starts_with('0') { "f" } else { "0" };
    forged.replace_range(0..1, replacement);
    assert_ne!(forged, orig_sig, "改ざん後の署名が元と同じになっている");
    j["author_sig"] = serde_json::Value::String(forged);

    // entry_id(=ファイル名かつフィールド)は変えない → ハッシュは一致するはず。
    fs::write(&path, serde_json::to_string_pretty(&j).unwrap()).expect("書き戻しに失敗");

    let reloaded = SemanticCache::new(store, &embedder, &signer, LOCAL_THRESHOLD);
    assert_eq!(reloaded.size(), 0, "author_sig 改ざんが署名検証で弾かれなかった");
}

#[test]
fn exact_match_hits_with_similarity_near_one()
{
    // HIT: 登録した質問と完全一致 → Some を返し similarity は約 1.0。
    let dir = super::common::temp_dir("cache_hit");
    let store = dir.join("store");
    let keyfile = dir.join("node.key");

    let embedder = MockEmbedder::default();
    let signer = DummySigner::new(&keyfile).expect("signer 初期化に失敗");
    let mut cache = SemanticCache::new(store, &embedder, &signer, LOCAL_THRESHOLD);

    let q = "水の沸点は摂氏何度ですか";
    cache.register_entry(q, "1気圧では摂氏100度です", "slow", true, "共有可", "mock-agent");

    let r = cache.lookup(q);
    assert!(r.entry.is_some(), "完全一致がヒットしなかった");
    assert!(
        r.similarity >= 0.999,
        "完全一致の類似度が想定より低い: {}",
        r.similarity
    );
    assert_eq!(r.entry.unwrap().question, q);
}

#[test]
fn empty_cache_misses()
{
    // MISS(a): 空キャッシュでは entry は None。
    let dir = super::common::temp_dir("cache_miss_empty");
    let store = dir.join("store");
    let keyfile = dir.join("node.key");

    let embedder = MockEmbedder::default();
    let signer = DummySigner::new(&keyfile).expect("signer 初期化に失敗");
    let cache = SemanticCache::new(store, &embedder, &signer, LOCAL_THRESHOLD);

    assert_eq!(cache.size(), 0);
    let r = cache.lookup("水の沸点は摂氏何度ですか");
    assert!(r.entry.is_none(), "空キャッシュがヒットを返した");
}

#[test]
fn dissimilar_query_misses_below_threshold()
{
    // MISS(b): 登録済みだが全く異なる質問はしきい値未満で None。
    //   MockEmbedder は文字 n-gram ベースなので、共有 n-gram を持たない
    //   文字列同士は類似度がほぼ 0 になる。
    let dir = super::common::temp_dir("cache_miss_far");
    let store = dir.join("store");
    let keyfile = dir.join("node.key");

    let embedder = MockEmbedder::default();
    let signer = DummySigner::new(&keyfile).expect("signer 初期化に失敗");
    let mut cache = SemanticCache::new(store, &embedder, &signer, LOCAL_THRESHOLD);

    cache.register_entry(
        "水の沸点は摂氏何度ですか",
        "1気圧では摂氏100度です",
        "slow",
        true,
        "共有可",
        "mock-agent",
    );

    let r = cache.lookup("zzzzz qqqqq wwwww kkkkk");
    assert!(
        r.entry.is_none(),
        "無関係な質問がヒットしてしまった (sim={})",
        r.similarity
    );
    assert!(
        r.similarity < LOCAL_THRESHOLD,
        "無関係な質問の類似度がしきい値以上: {}",
        r.similarity
    );
}
