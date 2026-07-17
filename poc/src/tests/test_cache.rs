// SemanticCache のテスト(cache.rs / S2.5 エントリ形式)。
//
// S2.5(docs/S2.5_エントリ形式設計.md §9 テスト観点表)を全てカバーする:
//   - 正準決定性 / ゴールデン16進スナップショット / encode↔parse round-trip
//   - 改ざん検知の2経路分離(ハッシュ不一致=検知 / 署名検証失敗=偽造防止。設計メモ §4)
//   - 可変状態(state.json)の entry_id 非影響
//   - 受信側再導出(disk 上の shareable 偽装がロード後に無視される。§6 手順8)
//   - question_key による重複排除と版併存 / NFC 正規化 / Tier round-trip
//   - embedding 非保存(改良案C)/ core に浮動小数を含まない構造保証
//   - HIT / MISS のしきい値挙動
//
// 保存レイアウト(§6): <entry_id>.entry(不変・serde JSON エンベロープ。中の
// core_b64 のみ authoritative)+ <entry_id>.state.json(可変・署名対象外)。
//
// 注意: SemanticCache は embedder / signer を参照で保持するため、
//       これらは cache より先に束縛して長生きさせる。

use crate::cache::{
    derive_operative_state, encode_core, fold, parse_core, question_key, ImmutableCore,
    Provenance, SemanticCache, Tier, LOCAL_THRESHOLD,
};
use crate::triples::FactTriple;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

// ------------------------------------------------------------------
// ゴールデン基準値(§9「ゴールデン」観点。C#実装のクロス参照基準)
//
// 下記の固定 ImmutableCore(created を固定文字列にピン留め)を encode_core した
// 正準バイト列の16進、およびその sha256(= entry_id)。実装が生成する実際の値を
// 一度実行して取得し、期待値として固定してある。encode_core のバイト順・
// domain_tag("nyllm/entry/v1\n" = 実15バイト)・長さ接頭辞方式が変わると
// この値が動く = C#実装との互換が壊れることを検出する。
// ------------------------------------------------------------------
const GOLDEN_CORE_HEX: &str = "6e796c6c6d2f656e7472792f76310a0001000000176e69686f6e6e6f2d736875746f2d776120746f7061636500000001000000056e69686f6e000000077368757475303a000000077368757475303a0000000a6d6f636b2d6167656e74000000000000000f6d6f636b2d6e6772616d2d6861736800000014323032362d30372d31375430303a30303a30305a000000097065726d616e656e7400";
const GOLDEN_ENTRY_ID: &str = "64386d98f2a5542bfabfcaeee2204529cdc9c6d1c9053f55a6c11288fd16875e";

// テスト内で独立に sha256(hex) を計算する(cache.rs 内部関数に依存しない)。
fn sha256_hex_local(data: &[u8]) -> String
{
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

// store_dir 内の最初の .entry(署名済みエンベロープ)のパスを返す。
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

// entry_id(= .entry のファイルステム)に対応する state.json のパス。
fn state_path_for(entry_file: &Path) -> PathBuf
{
    let id = entry_file.file_stem().unwrap().to_string_lossy().to_string();
    entry_file.with_file_name(format!("{id}.state.json"))
}

// ゴールデン基準にも使う固定コア(created をピン留め)。
fn golden_core() -> ImmutableCore
{
    ImmutableCore
    {
        schema_ver: 1,
        question_norm: "nihonno-shuto-wa topace".to_string(),
        facts: vec![FactTriple
        {
            s: "nihon".to_string(),
            p: "shutu0:".to_string(),
            o: "shutu0:".to_string(),
        }],
        provenance: Provenance
        {
            agent: "mock-agent".to_string(),
            model: String::new(),
            embedder_model_id: "mock-ngram-hash".to_string(),
        },
        created: "2026-07-17T00:00:00Z".to_string(),
        initial_volatility_class: "permanent".to_string(),
        initial_tier: Tier::Low,
    }
}

#[test]
fn encode_core_is_deterministic_and_id_matches()
{
    // 正準決定性: 同一 ImmutableCore を encode_core で2回 → バイト完全一致
    // → entry_id(sha256)一致。register が使う entry_id も core_bytes の sha256。
    let core = golden_core();
    let b1 = encode_core(&core);
    let b2 = encode_core(&core);
    assert_eq!(b1, b2, "同一コアの encode_core が2回で一致しない(非決定的)");
    assert_eq!(
        sha256_hex_local(&b1),
        sha256_hex_local(&b2),
        "同一 core_bytes の entry_id が一致しない"
    );
}

#[test]
fn golden_core_bytes_snapshot()
{
    // ゴールデン: 固定コアの core_bytes 16進と entry_id をピン留め。
    // C#実装が同一入力から同一バイト列を再現する基準(§3, §9)。
    let core = golden_core();
    let bytes = encode_core(&core);
    let core_hex = hex::encode(&bytes);
    let entry_id = sha256_hex_local(&bytes);
    // 実行時に実値を目視できるよう出力(--nocapture)。
    println!("GOLDEN core_hex = {core_hex}");
    println!("GOLDEN entry_id = {entry_id}");

    assert_eq!(core_hex, GOLDEN_CORE_HEX, "core_bytes 16進がゴールデンと不一致");
    assert_eq!(entry_id, GOLDEN_ENTRY_ID, "entry_id がゴールデンと不一致");

    // 先頭 15 バイトは domain_tag "nyllm/entry/v1\n"(実15バイト。長さ接頭辞なし)。
    assert_eq!(&bytes[..15], b"nyllm/entry/v1\n", "domain_tag が不一致");
}

#[test]
fn encode_parse_round_trip_struct()
{
    // round-trip: encode_core → parse_core → 同一構造体。
    let core = golden_core();
    let bytes = encode_core(&core);
    let parsed = parse_core(&bytes).expect("parse_core が失敗");
    assert_eq!(parsed, core, "encode→parse で構造体が一致しない");
    // parse_core(encode_core(x)) を再エンコードするとバイトも一致(正準性)。
    assert_eq!(encode_core(&parsed), bytes, "再エンコードでバイトが一致しない");
}

#[test]
fn round_trip_through_base64_envelope()
{
    // base64 エンベロープ経由(register → save → load)でも core が一致する。
    let dir = super::common::temp_dir("cache_env_roundtrip");
    let store = dir.join("store");
    let keyfile = dir.join("node.key");

    let embedder = crate::embedder::MockEmbedder::default();
    let signer = crate::signer::DummySigner::new(&keyfile).expect("signer 初期化に失敗");

    let (orig_id, orig_qnorm);
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
        orig_id = e.entry_id.clone();
        orig_qnorm = e.core.question_norm.clone();
        // entry_id は core_bytes の sha256 と一致する(内容アドレス)。
        assert_eq!(e.entry_id, sha256_hex_local(&e.core_bytes));
    }

    let reloaded = SemanticCache::new(store, &embedder, &signer, LOCAL_THRESHOLD);
    assert_eq!(reloaded.size(), 1, "エンベロープ経由の再ロードで件数が変化");
    let r = reloaded.lookup("水の沸点は摂氏何度ですか");
    let e = r.entry.expect("再ロード後にヒットしない");
    assert_eq!(e.entry_id, orig_id, "再ロードで entry_id が変化");
    assert_eq!(e.core.question_norm, orig_qnorm, "再ロードで question_norm が変化");
}

#[test]
fn valid_entry_survives_reload()
{
    // ポジティブコントロール: 改ざんしなければ再ロードでも生き残る。
    let dir = super::common::temp_dir("cache_reload_ok");
    let store = dir.join("store");
    let keyfile = dir.join("node.key");

    let embedder = crate::embedder::MockEmbedder::default();
    let signer = crate::signer::DummySigner::new(&keyfile).expect("signer 初期化に失敗");

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

    let reloaded = SemanticCache::new(store, &embedder, &signer, LOCAL_THRESHOLD);
    assert_eq!(reloaded.size(), 1, "正常エントリが再ロードで失われた");
}

#[test]
fn tampered_core_dropped_by_hash_mismatch()
{
    // 改ざん検知 経路(a): 保存済み .entry の core_b64 を復号し question_norm 相当の
    // 1バイトを書き換えて再 base64 → ロードで sha256(core_bytes) != ファイル名 となり
    // 「ハッシュ不一致=改ざん検知」で drop される(§6 手順3)。
    // これは下の author_sig テスト(経路(b) = 署名検証失敗=偽造防止)とは別経路
    // (設計メモ §4: ハッシュ=検知 / 署名=偽造防止 を混同しない)。
    let dir = super::common::temp_dir("cache_tamper_hash");
    let store = dir.join("store");
    let keyfile = dir.join("node.key");

    let embedder = crate::embedder::MockEmbedder::default();
    let signer = crate::signer::DummySigner::new(&keyfile).expect("signer 初期化に失敗");

    {
        let mut cache = SemanticCache::new(store.clone(), &embedder, &signer, LOCAL_THRESHOLD);
        cache.register_entry(
            "waterwaterwater",
            "1気圧では摂氏100度です",
            "slow",
            true,
            "共有可",
            "mock-agent",
        );
        assert_eq!(cache.size(), 1);
    }

    // core_b64 を復号 → question_norm のバイト列("waterwaterwater")の1バイトを
    // 反転 → 再 base64 でエンベロープに書き戻す(ファイル名 = 旧 entry_id のまま)。
    let path = first_entry_file(&store);
    let data = fs::read_to_string(&path).expect("エントリ読込に失敗");
    let mut j: serde_json::Value = serde_json::from_str(&data).expect("JSON パースに失敗");
    let core_b64 = j["core_b64"].as_str().expect("core_b64 が無い").to_string();
    let mut core_bytes = B64.decode(&core_b64).expect("core_b64 復号に失敗");
    // "waterwaterwater" の先頭バイト 'w'(0x77) を探して反転する。
    let needle = b"waterwaterwater";
    let pos = core_bytes
        .windows(needle.len())
        .position(|w| w == needle)
        .expect("question_norm のバイト列が core に見つからない");
    core_bytes[pos] ^= 0xFF; // question_norm の1バイトを書き換え(内容改ざん)
    j["core_b64"] = serde_json::Value::String(B64.encode(&core_bytes));
    fs::write(&path, serde_json::to_string_pretty(&j).unwrap()).expect("書き戻しに失敗");

    let reloaded = SemanticCache::new(store, &embedder, &signer, LOCAL_THRESHOLD);
    assert_eq!(reloaded.size(), 0, "core 改ざんがハッシュ不一致で検知されなかった");
}

#[test]
fn tampered_signature_dropped_by_verify_failure()
{
    // 改ざん検知 経路(b): author_sig だけを書き換える。core_bytes は無傷なので
    // ハッシュ(entry_id)は一致したままだが、署名検証(HMAC 再計算)が失敗して
    // drop される = 「署名検証失敗=偽造防止」(§6 手順4)。経路(a)とは独立
    // (設計メモ §4)。
    let dir = super::common::temp_dir("cache_tamper_sig");
    let store = dir.join("store");
    let keyfile = dir.join("node.key");

    let embedder = crate::embedder::MockEmbedder::default();
    let signer = crate::signer::DummySigner::new(&keyfile).expect("signer 初期化に失敗");

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

    let path = first_entry_file(&store);
    let data = fs::read_to_string(&path).expect("エントリ読込に失敗");
    let mut j: serde_json::Value = serde_json::from_str(&data).expect("JSON パースに失敗");
    let mut forged: String = orig_sig.clone();
    let replacement = if forged.starts_with('0') { "f" } else { "0" };
    forged.replace_range(0..1, replacement);
    assert_ne!(forged, orig_sig, "改ざん後の署名が元と同じ");
    j["author_sig"] = serde_json::Value::String(forged);
    // core_b64(= ファイル名の元)は触らない → ハッシュは一致するはず。
    fs::write(&path, serde_json::to_string_pretty(&j).unwrap()).expect("書き戻しに失敗");

    let reloaded = SemanticCache::new(store, &embedder, &signer, LOCAL_THRESHOLD);
    assert_eq!(reloaded.size(), 0, "author_sig 改ざんが署名検証で弾かれなかった");
}

#[test]
fn mutable_state_edit_does_not_change_entry_id()
{
    // 可変状態の非影響: state.json の confidence / shareable を書き換えても
    // entry_id は不変(core と分離されている証明)。エントリは生き残り、
    // entry_id も同一のまま。disk の confidence は §6 手順9 で採用されるので
    // 反映され、shareable は再導出値が採用される(下の別テストで確認)。
    let dir = super::common::temp_dir("cache_state_noid");
    let store = dir.join("store");
    let keyfile = dir.join("node.key");

    let embedder = crate::embedder::MockEmbedder::default();
    let signer = crate::signer::DummySigner::new(&keyfile).expect("signer 初期化に失敗");

    let orig_id;
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
        orig_id = e.entry_id.clone();
    }

    let entry_file = first_entry_file(&store);
    let spath = state_path_for(&entry_file);
    let sdata = fs::read_to_string(&spath).expect("state.json 読込に失敗");
    let mut s: serde_json::Value = serde_json::from_str(&sdata).expect("state JSON パース失敗");
    s["volatility_confidence"] = serde_json::json!(0.01);
    s["shareable"] = serde_json::json!(false);
    fs::write(&spath, serde_json::to_string_pretty(&s).unwrap()).expect("state 書き戻し失敗");

    // .entry のファイル名(= entry_id)は変わっていないはず。
    let entry_file_after = first_entry_file(&store);
    let id_after = entry_file_after.file_stem().unwrap().to_string_lossy().to_string();
    assert_eq!(id_after, orig_id, "state 編集で .entry ファイル名(entry_id)が変わった");

    let reloaded = SemanticCache::new(store, &embedder, &signer, LOCAL_THRESHOLD);
    assert_eq!(reloaded.size(), 1, "state 編集でエントリが落ちた(core と非分離)");
    let e = reloaded.lookup("水の沸点は摂氏何度ですか").entry.expect("ヒットせず");
    assert_eq!(e.entry_id, orig_id, "entry_id が変化した");
    // disk の confidence は再導出値ではなくそのまま採用される(§6 手順9)。
    assert!((e.state.volatility_confidence - 0.01).abs() < 1e-6, "disk confidence が反映されていない");
}

#[test]
fn forged_shareable_in_state_is_re_derived_on_load()
{
    // 受信側再導出(§6 手順8): state.json の shareable=true を偽装しても、
    // ロード後は derive_operative_state による再導出値が採用され偽装値は無視される。
    // volatile な質問を使うため、再導出結果は必ず shareable=false になる
    // (送信者の主張を一切信頼しない、の実装形)。
    let dir = super::common::temp_dir("cache_forge_shareable");
    let store = dir.join("store");
    let keyfile = dir.join("node.key");

    let embedder = crate::embedder::MockEmbedder::default();
    let signer = crate::signer::DummySigner::new(&keyfile).expect("signer 初期化に失敗");

    let q = "最新の株価はいくらですか"; // "最新"/"株価" = volatile 語彙
    {
        let mut cache = SemanticCache::new(store.clone(), &embedder, &signer, LOCAL_THRESHOLD);
        // 登録側では shareable=false(volatile)だが、あえて disk を true に偽装する。
        cache.register_entry(q, "現在の株価は不明です", "volatile", false, "volatile", "mock-agent");
    }

    let entry_file = first_entry_file(&store);
    let spath = state_path_for(&entry_file);
    let sdata = fs::read_to_string(&spath).expect("state.json 読込に失敗");
    let mut s: serde_json::Value = serde_json::from_str(&sdata).expect("state JSON パース失敗");
    s["shareable"] = serde_json::json!(true); // 偽装
    s["share_reason"] = serde_json::json!("FORGED shareable");
    fs::write(&spath, serde_json::to_string_pretty(&s).unwrap()).expect("state 書き戻し失敗");

    let reloaded = SemanticCache::new(store, &embedder, &signer, LOCAL_THRESHOLD);
    let e = reloaded.lookup(q).entry.expect("ヒットせず");
    assert!(!e.state.shareable, "偽装 shareable=true がロード後も残った(再導出されていない)");
    assert!(
        e.state.share_reason.contains("再導出"),
        "share_reason が再導出由来でない: {}",
        e.state.share_reason
    );
}

#[test]
fn question_key_dedup_and_version_coexistence()
{
    // question_key 重複排除: fold 同値(大小文字/連続空白違い)の2質問は同一 question_key。
    assert_eq!(
        question_key("Hello   World"),
        question_key("hello world"),
        "fold 同値の質問で question_key が一致しない"
    );
    assert_eq!(fold("Hello   World"), "hello world", "fold の畳み込みが想定外");

    // 異版(created 違い)は別 entry_id で併存する(§5.2)。question_key は同一。
    let mut c1 = golden_core();
    let mut c2 = golden_core();
    c1.created = "2026-07-17T00:00:00Z".to_string();
    c2.created = "2026-07-18T00:00:00Z".to_string();
    let id1 = sha256_hex_local(&encode_core(&c1));
    let id2 = sha256_hex_local(&encode_core(&c2));
    assert_ne!(id1, id2, "created 違いで entry_id が同一になった(版が区別されない)");

    // 同一質問文なら question_key は created に依存せず同一(識別層 = content-based)。
    let qk = question_key(&c1.question_norm);
    assert_eq!(qk, question_key(&c2.question_norm), "同一質問で question_key が一致しない");
}

#[test]
fn nfc_normalization_yields_same_id_and_key()
{
    // NFC 正規化: 合成済み("が" = U+304C)と分解済み("か"+濁点 = U+304B U+3099)の
    // 同一質問は、NFC 後に一致するため同一 question_key / 同一 entry_id になる。
    let composed = "\u{304C}"; // が
    let decomposed = "\u{304B}\u{3099}"; // か + 結合濁点 → NFC → が
    assert_ne!(composed, decomposed, "前提: 合成/分解の生バイトは異なる");

    // question_key(fold 内で NFC される)
    assert_eq!(
        question_key(composed),
        question_key(decomposed),
        "NFC 差異で question_key が割れた"
    );

    // entry_id(encode_core 内の push_lp_str で NFC される)
    let mut c_comp = golden_core();
    let mut c_dec = golden_core();
    c_comp.question_norm = composed.to_string();
    c_dec.question_norm = decomposed.to_string();
    assert_eq!(
        sha256_hex_local(&encode_core(&c_comp)),
        sha256_hex_local(&encode_core(&c_dec)),
        "NFC 差異で entry_id が割れた"
    );
}

#[test]
fn tier_round_trips_and_re_derives()
{
    // Tier round-trip: initial_tier=High が u8=1 で正準化 → parse → 同値。
    let mut core = golden_core();
    core.initial_tier = Tier::High;
    let bytes = encode_core(&core);
    // 正準表現の末尾1バイトが tier(High=1)。
    assert_eq!(*bytes.last().unwrap(), 1u8, "initial_tier=High の u8 表現が 1 でない");
    let parsed = parse_core(&bytes).expect("parse 失敗");
    assert_eq!(parsed.initial_tier, Tier::High, "Tier round-trip が壊れている");

    // tier_operative の再導出(Phase1 は initial_tier をそのまま運用値に置く)。
    let derived = derive_operative_state(&core);
    assert_eq!(derived.tier_operative, Tier::High, "tier_operative 再導出が initial_tier と不一致");
}

#[test]
fn embedding_is_not_persisted_but_recomputed_on_load()
{
    // embedding 非保存(改良案C): .entry / .state.json のどちらにも "embedding" が
    // 現れない。ロード時に自ノードの embedder で再計算され、lookup がヒットする。
    let dir = super::common::temp_dir("cache_no_embedding");
    let store = dir.join("store");
    let keyfile = dir.join("node.key");

    let embedder = crate::embedder::MockEmbedder::default();
    let signer = crate::signer::DummySigner::new(&keyfile).expect("signer 初期化に失敗");

    let q = "水の沸点は摂氏何度ですか";
    {
        let mut cache = SemanticCache::new(store.clone(), &embedder, &signer, LOCAL_THRESHOLD);
        cache.register_entry(q, "1気圧では摂氏100度です", "slow", true, "共有可", "mock-agent");
    }

    let entry_file = first_entry_file(&store);
    let entry_txt = fs::read_to_string(&entry_file).expect(".entry 読込失敗");
    let state_txt = fs::read_to_string(state_path_for(&entry_file)).expect("state 読込失敗");
    assert!(!entry_txt.contains("embedding"), ".entry に embedding が保存されている");
    assert!(!state_txt.contains("embedding"), ".state.json に embedding が保存されている");

    // ロード時に再計算され、完全一致でヒットする(sim ~ 1.0)。
    let reloaded = SemanticCache::new(store, &embedder, &signer, LOCAL_THRESHOLD);
    let r = reloaded.lookup(q);
    assert!(r.entry.is_some(), "再ロード後(embedding 再計算)にヒットしない");
    assert!(r.similarity >= 0.999, "再計算 embedding の完全一致類似度が低い: {}", r.similarity);
}

#[test]
fn no_float_in_core_byte_layout()
{
    // no-float-in-core(§9): ImmutableCore は f32/f64 フィールドを持たない。
    // 型に浮動小数がないことを、正準バイト列の長さが「長さ接頭辞文字列 + 固定幅整数
    // (u8/u16/u32)」の合計に厳密一致することで裏づける。もし core に浮動小数が
    // 追加されれば、この算術和(浮動小数のバイトを含まない)は実バイト長と食い違う。
    let core = golden_core();
    let bytes = encode_core(&core);

    // lp_str = 4(u32長) + NFC後UTF-8バイト長。core は全て NFC 済み ASCII/かな。
    fn lp_len(s: &str) -> usize
    {
        4 + s.len() // golden_core は既に NFC 正規形なので nfc(s).len() == s.len()
    }
    let mut expected = 0usize;
    expected += b"nyllm/entry/v1\n".len(); // domain_tag(長さ接頭辞なし)
    expected += 2; // schema_ver: u16
    expected += lp_len(&core.question_norm);
    expected += 4; // facts count: u32
    for t in &core.facts
    {
        expected += lp_len(&t.s) + lp_len(&t.p) + lp_len(&t.o);
    }
    expected += lp_len(&core.provenance.agent);
    expected += lp_len(&core.provenance.model);
    expected += lp_len(&core.provenance.embedder_model_id);
    expected += lp_len(&core.created);
    expected += lp_len(&core.initial_volatility_class);
    expected += 1; // initial_tier: u8

    assert_eq!(
        bytes.len(),
        expected,
        "core_bytes 長が整数+長さ接頭辞文字列の合計と不一致(浮動小数混入の疑い)"
    );
}

#[test]
fn exact_match_hits_with_similarity_near_one()
{
    // HIT: 登録した質問と完全一致 → Some を返し similarity は約 1.0。
    let dir = super::common::temp_dir("cache_hit");
    let store = dir.join("store");
    let keyfile = dir.join("node.key");

    let embedder = crate::embedder::MockEmbedder::default();
    let signer = crate::signer::DummySigner::new(&keyfile).expect("signer 初期化に失敗");
    let mut cache = SemanticCache::new(store, &embedder, &signer, LOCAL_THRESHOLD);

    let q = "水の沸点は摂氏何度ですか";
    cache.register_entry(q, "1気圧では摂氏100度です", "slow", true, "共有可", "mock-agent");

    let r = cache.lookup(q);
    assert!(r.entry.is_some(), "完全一致がヒットしなかった");
    assert!(r.similarity >= 0.999, "完全一致の類似度が想定より低い: {}", r.similarity);
    assert_eq!(r.entry.unwrap().core.question_norm, q);
}

#[test]
fn empty_cache_misses()
{
    // MISS(a): 空キャッシュでは entry は None。
    let dir = super::common::temp_dir("cache_miss_empty");
    let store = dir.join("store");
    let keyfile = dir.join("node.key");

    let embedder = crate::embedder::MockEmbedder::default();
    let signer = crate::signer::DummySigner::new(&keyfile).expect("signer 初期化に失敗");
    let cache = SemanticCache::new(store, &embedder, &signer, LOCAL_THRESHOLD);

    assert_eq!(cache.size(), 0);
    let r = cache.lookup("水の沸点は摂氏何度ですか");
    assert!(r.entry.is_none(), "空キャッシュがヒットを返した");
}

#[test]
fn dissimilar_query_misses_below_threshold()
{
    // MISS(b): 登録済みだが全く異なる質問はしきい値未満で None。
    let dir = super::common::temp_dir("cache_miss_far");
    let store = dir.join("store");
    let keyfile = dir.join("node.key");

    let embedder = crate::embedder::MockEmbedder::default();
    let signer = crate::signer::DummySigner::new(&keyfile).expect("signer 初期化に失敗");
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
    assert!(r.entry.is_none(), "無関係な質問がヒットしてしまった (sim={})", r.similarity);
    assert!(r.similarity < LOCAL_THRESHOLD, "無関係な質問の類似度がしきい値以上: {}", r.similarity);
}
