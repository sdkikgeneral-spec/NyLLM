// M-2【shareable 単調性保護】の回帰テスト(脅威レビュー指摘 M-2 / cache.rs load() 手順9)。
//
// 修正の骨子(実装本体 cache.rs::load 手順9):
//   reload 時、shareable は「再導出値(手順8)AND ディスク state.json の値(cap)」
//   で合成する。derive_operative_state は answer 平文と Agent を持たないため
//   judge_entry(登録時の全段ANDゲート)より緩く、再導出単独だと reload で
//   shareable が false→true に反転しうる(非単調)。S3 で shareable が伝播ゲート
//   (供出/Digest/announce)に配線されたため、この反転を放置すると judge_entry が
//   共有不可とした緩いエントリが reload を経て網へ漏れる。よって:
//     - disk.shareable=false は再導出が true でも false へ抑える(緩めない)
//     - disk.shareable=true は再導出値をそのまま採用(false のものを true にしない)
//     - state.json 不在/破損は登録時判定を確認できない → 保守側(false)
//   ネット越し受信(ingest)はそもそも state を運ばず、送信者申告 shareable を
//   持たない。受信側は再導出のみで決める(disk cap も送信者値も介在しない)。

use super::common::{new_signer, shared_embedder, temp_dir, triple};
use crate::cache::{SemanticCache, LOCAL_THRESHOLD};
use crate::entry::ImmutableCore;
use crate::signer::Signer;
use crate::triples::FactTriple;
use crate::volatility::VolatilityAssessment;
use std::path::Path;
use std::sync::Arc;

// ------------------------------------------------------------------
// ローカルヘルパー(同一 store_dir・同一鍵ファイルで開き直す = reload)
// ------------------------------------------------------------------

// 同一鍵ファイルから signer を復元しつつ store_dir を開き直す。
// (DummySigner は自鍵でのみ verify 可能なので、reload でも同一鍵が必須)
fn open_cache(dir: &Path, sub: &str) -> SemanticCache
{
    let signer: Arc<dyn Signer> = new_signer(dir, "n1");
    // SemanticCache::new(store_dir, embedder, signer, threshold)
    SemanticCache::new(dir.join(sub), shared_embedder(), signer, LOCAL_THRESHOLD)
}

fn permanent_assessment() -> VolatilityAssessment
{
    VolatilityAssessment
    {
        class: "permanent".to_string(),
        confidence: 0.9,
        evidence: vec!["test".to_string()],
    }
}

// 首都 core(再導出で shareable=true になる既知内容)。
fn capital_facts() -> Vec<FactTriple>
{
    vec![triple("日本", "首都", "東京")]
}

// 為替 core(再導出で volatile → shareable=false になる既知内容)。
fn fx_facts() -> Vec<FactTriple>
{
    vec![triple("ドル", "為替レート", "150円")]
}

// state.json の shareable フィールドだけをディスク上で書き換える
// (他フィールドは温存。serde_json::Value 経由で堅牢に)。
fn overwrite_disk_shareable(dir: &Path, sub: &str, entry_id: &str, value: bool)
{
    let path = dir.join(sub).join(format!("{entry_id}.state.json"));
    let data = std::fs::read_to_string(&path).expect("state.json 読み込み");
    let mut v: serde_json::Value = serde_json::from_str(&data).expect("state.json パース");
    v["shareable"] = serde_json::json!(value);
    std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).expect("state.json 書き戻し");
}

fn state_json_path(dir: &Path, sub: &str, entry_id: &str) -> std::path::PathBuf
{
    dir.join(sub).join(format!("{entry_id}.state.json"))
}

// ------------------------------------------------------------------
// disk=false は再導出 true を false へ抑える(緩めない)
// ------------------------------------------------------------------

#[test]
fn disk_false_caps_rederived_true_to_false()
{
    let dir = temp_dir("m2_false_caps_true");
    let eid;
    {
        let mut cache = open_cache(&dir, "store");
        // 首都エントリを共有可(true)で登録 → state.json は shareable=true。
        let e = cache.register(
            "日本の首都はどこですか",
            "日本の首都は東京です。",
            &permanent_assessment(),
            &capital_facts(),
            true,
            "初期共有可",
            "mock",
        );
        eid = e.entry_id.clone();
    }
    // ディスクの state.json を shareable=false に改変(登録時判定=共有不可の状況を再現)。
    overwrite_disk_shareable(&dir, "store", &eid, false);

    // reload: 再導出は true(首都 permanent)だが、disk cap=false で false に抑えられる。
    let reloaded = open_cache(&dir, "store");
    let e = reloaded.get(&eid).expect("エントリはロードされる(core は検証済み)");
    assert!(
        !e.state.shareable,
        "disk=false は再導出=true を false へ抑える(緩めない)"
    );
    assert!(
        e.state.share_reason.contains("M-2単調性保護"),
        "M-2 保護の印が付く: {}",
        e.state.share_reason
    );
}

// ------------------------------------------------------------------
// disk=true は再導出値をそのまま採用(正当な共有可を過剰抑制しない)
// ------------------------------------------------------------------

#[test]
fn disk_true_keeps_rederived_true()
{
    let dir = temp_dir("m2_true_keeps_true");
    let eid;
    {
        let mut cache = open_cache(&dir, "store");
        let e = cache.register(
            "日本の首都はどこですか",
            "日本の首都は東京です。",
            &permanent_assessment(),
            &capital_facts(),
            true, // disk=true
            "初期共有可",
            "mock",
        );
        eid = e.entry_id.clone();
    }
    // 改変せず reload: 再導出 true AND disk true = true。
    let reloaded = open_cache(&dir, "store");
    let e = reloaded.get(&eid).expect("ロードされる");
    assert!(
        e.state.shareable,
        "disk=true かつ再導出=true は共有可のまま(保護が過剰抑制しない)"
    );
}

// ------------------------------------------------------------------
// 逆方向: disk=true でも再導出=false なら false(disk が緩めない)
// ------------------------------------------------------------------

#[test]
fn disk_true_cannot_loosen_rederived_false()
{
    let dir = temp_dir("m2_true_cannot_loosen");
    let eid;
    {
        let mut cache = open_cache(&dir, "store");
        // 為替(volatile)エントリを、ディスクには shareable=true で置く。
        let e = cache.register(
            "ドルの為替レートはいくらですか",
            "ドルの為替レートは150円です。",
            &permanent_assessment(), // 著者は permanent と主張(=偽)
            &fx_facts(),
            true, // disk=true(送信者/登録時が共有可と主張した状況)
            "主張:共有可",
            "mock",
        );
        eid = e.entry_id.clone();
    }
    // reload: 再導出は volatile → false。disk=true でも AND で false のまま。
    let reloaded = open_cache(&dir, "store");
    let e = reloaded.get(&eid).expect("ロードされる");
    assert_eq!(
        e.state.volatility_class_operative, "volatile",
        "為替は volatile と再導出される"
    );
    assert!(
        !e.state.shareable,
        "再導出=false は disk=true でも false のまま(disk は緩める方向に作用しない)"
    );
}

// ------------------------------------------------------------------
// state.json 不在 → 登録時判定を確認できず保守側(false)
// ------------------------------------------------------------------

#[test]
fn missing_state_json_forces_shareable_false()
{
    let dir = temp_dir("m2_missing_state");
    let eid;
    {
        let mut cache = open_cache(&dir, "store");
        let e = cache.register(
            "日本の首都はどこですか",
            "日本の首都は東京です。",
            &permanent_assessment(),
            &capital_facts(),
            true,
            "初期共有可",
            "mock",
        );
        eid = e.entry_id.clone();
    }
    // state.json を削除(.entry は残す)。
    std::fs::remove_file(state_json_path(&dir, "store", &eid)).expect("state.json 削除");

    let reloaded = open_cache(&dir, "store");
    let e = reloaded.get(&eid).expect("core は検証済みなのでロードは成功する");
    assert!(
        !e.state.shareable,
        "state.json 不在では登録時判定を確認できないため共有保留(false)"
    );
    assert!(
        e.state.share_reason.contains("不在") || e.state.share_reason.contains("M-2単調性保護"),
        "不在で共有保留した印: {}",
        e.state.share_reason
    );
}

// ------------------------------------------------------------------
// state.json 破損 → パース失敗でも core は生かしつつ shareable=false
// ------------------------------------------------------------------

#[test]
fn corrupt_state_json_forces_shareable_false_but_keeps_entry()
{
    let dir = temp_dir("m2_corrupt_state");
    let eid;
    {
        let mut cache = open_cache(&dir, "store");
        let e = cache.register(
            "日本の首都はどこですか",
            "日本の首都は東京です。",
            &permanent_assessment(),
            &capital_facts(),
            true,
            "初期共有可",
            "mock",
        );
        eid = e.entry_id.clone();
    }
    // state.json を壊す(JSON として不正)。
    std::fs::write(state_json_path(&dir, "store", &eid), b"{ this is not valid json ]]")
        .expect("破損 state.json 書き込み");

    let reloaded = open_cache(&dir, "store");
    let e = reloaded.get(&eid).expect("破損 state でも core は生きる(entry は drop しない)");
    assert!(
        !e.state.shareable,
        "state.json 破損では cap 不明のため共有保留(false)"
    );
}

// ------------------------------------------------------------------
// ネット越し受信は再導出のみ(disk cap も送信者申告 shareable も介在しない)
// ------------------------------------------------------------------

#[test]
fn network_ingest_uses_rederivation_only_no_sender_shareable()
{
    use super::common::{envelope_from_core, make_core};
    use crate::entry::{encode_core, entry_id, Tier};

    let dir = temp_dir("m2_network_rederive");
    let signer: Arc<dyn Signer> = new_signer(&dir, "n1");
    let mut cache =
        SemanticCache::new(dir.join("store"), shared_embedder(), signer.clone(), LOCAL_THRESHOLD);

    // 送信エンベロープ(EntryEnvelope)は core+署名のみで shareable フィールドを
    // 一切持たない = 送信者は shareable を主張できない(構造的保証)。
    // 首都 core → 受信側再導出で true。
    let cap: ImmutableCore = make_core(
        "日本の首都はどこですか",
        capital_facts(),
        "2026-07-18T00:00:00Z",
        "permanent",
        Tier::Low,
    );
    let cap_id = entry_id(&encode_core(&cap));
    cache
        .ingest_envelope(&envelope_from_core(&cap, signer.as_ref()), Some(&cap_id))
        .unwrap();
    assert!(
        cache.get(&cap_id).unwrap().state.shareable,
        "首都 core はネット越しでも再導出で共有可"
    );

    // 為替 core(著者は permanent と偽主張)→ 受信側再導出で volatile=false。
    let fx: ImmutableCore = make_core(
        "ドルの為替レートはいくらですか",
        fx_facts(),
        "2026-07-18T00:00:00Z",
        "permanent", // 偽の主張(送信者値)
        Tier::Low,
    );
    let fx_id = entry_id(&encode_core(&fx));
    cache
        .ingest_envelope(&envelope_from_core(&fx, signer.as_ref()), Some(&fx_id))
        .unwrap();
    assert!(
        !cache.get(&fx_id).unwrap().state.shareable,
        "為替 core は送信者の permanent 主張を無視し volatile 再導出で共有除外"
    );
}
