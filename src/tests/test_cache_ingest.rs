// cache.rs 受信側検証・冪等マージの単体テスト
// (S3設計ノート §3「受信側の検証手順」/ §4「一貫性と重複排除」)。
//
// ここでは NodeService(配送層)を挟まず SemanticCache 直下で
// verify_envelope / ingest_envelope / insert_verified を検証する。
// 組織PKI(手順2)は配送層の責務なので、本ファイルの対象外
// (署名検証コア=手順3〜9 に集中する)。
//
// 検証観点:
//   - IngestError の各系統(UnsupportedSchema / Base64Decode / HashMismatch /
//     BadSignature / MalformedCore)
//   - 冪等マージ(同一 entry_id → Duplicate、件数不変)
//   - 複数版併存(同一 question_key・異 created → 両保持・検索で新しい版)
//   - 受信側再導出(送信者が主張した state を信頼せず core から再導出する)
//   - マージの可換・冪等(順序を入れ替えても収束)

use super::common::{
    envelope_from_core, make_core, new_signer, rfc3339_days_ago, temp_dir, triple,
};
use crate::cache::{IngestError, IngestOutcome, SemanticCache, LOCAL_THRESHOLD};
use crate::entry::{encode_core, entry_id, EntryEnvelope, Tier};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;

// 首都エントリ(再導出で shareable=true になる既知 core)。
fn capital_core(created: &str) -> crate::entry::ImmutableCore
{
    make_core(
        "日本の首都はどこですか",
        vec![triple("日本", "首都", "東京")],
        created,
        "permanent",
        Tier::Low,
    )
}

fn fresh_cache(tag: &str) -> (SemanticCache, std::sync::Arc<dyn crate::signer::Signer>, std::path::PathBuf)
{
    let dir = temp_dir(tag);
    let signer = new_signer(&dir, "n1");
    let embedder = super::common::shared_embedder();
    let cache = SemanticCache::new(dir.join("store"), embedder, signer.clone(), LOCAL_THRESHOLD);
    (cache, signer, dir)
}

#[test]
fn ingest_valid_envelope_added_then_duplicate()
{
    let (mut cache, signer, _dir) = fresh_cache("ingest_valid");
    let core = capital_core(&rfc3339_days_ago(0));
    let env = envelope_from_core(&core, signer.as_ref());
    let id = entry_id(&encode_core(&core));

    // 1回目: Added。
    let r1 = cache.ingest_envelope(&env, Some(&id)).expect("検証成功のはず");
    assert_eq!(r1.outcome, IngestOutcome::Added);
    assert_eq!(cache.size(), 1);
    assert!(cache.contains(&id));

    // 2回目: 同一 entry_id は Duplicate(冪等・件数不変)。
    let r2 = cache.ingest_envelope(&env, Some(&id)).expect("検証は再度成功");
    assert_eq!(r2.outcome, IngestOutcome::Duplicate);
    assert_eq!(cache.size(), 1, "冪等マージで件数は増えない");
}

#[test]
fn receiver_rederives_shareable_true_from_core()
{
    // 送信者主張を持たない core(=Transfer 相当)から shareable=true を再導出。
    let (mut cache, signer, _dir) = fresh_cache("rederive_true");
    let core = capital_core(&rfc3339_days_ago(0));
    let env = envelope_from_core(&core, signer.as_ref());
    let id = entry_id(&encode_core(&core));

    cache.ingest_envelope(&env, Some(&id)).unwrap();
    let e = cache.get(&id).unwrap();
    assert!(e.state.shareable, "首都 permanent の core は再導出で shareable=true");
    assert!(
        e.state.share_reason.starts_with("[再導出]"),
        "share_reason は受信側再導出の印を持つ: {}",
        e.state.share_reason
    );
}

#[test]
fn receiver_ignores_lying_initial_class_and_derives_volatile()
{
    // 著者が initial_volatility_class="permanent" と偽っても、volatile 述語
    // (為替レート)を含む core は受信側が volatile と再導出し共有除外する。
    // = 送信者値を一切信頼しない(§3手順9)の実証。
    let (mut cache, signer, _dir) = fresh_cache("rederive_volatile");
    let core = make_core(
        "ドルの為替レートはいくらですか",
        vec![triple("ドル", "為替レート", "150円")],
        &rfc3339_days_ago(0),
        "permanent", // ← 偽の主張
        Tier::Low,
    );
    let env = envelope_from_core(&core, signer.as_ref());
    let id = entry_id(&encode_core(&core));

    cache.ingest_envelope(&env, Some(&id)).unwrap();
    let e = cache.get(&id).unwrap();
    assert_eq!(
        e.state.volatility_class_operative, "volatile",
        "volatile 述語は permanent 主張を無視して volatile 再導出"
    );
    assert!(!e.state.shareable, "volatile は共有除外(疑わしきは共有しない)");
    assert!(e.state.share_reason.starts_with("[再導出"));
}

#[test]
fn multiple_versions_coexist_same_question_key()
{
    // 同一 question_norm・異なる created → 同一 question_key・異なる entry_id。
    let (mut cache, signer, _dir) = fresh_cache("multiversion");
    let old = capital_core(&rfc3339_days_ago(10));
    let new = capital_core(&rfc3339_days_ago(1));
    let old_id = entry_id(&encode_core(&old));
    let new_id = entry_id(&encode_core(&new));
    assert_ne!(old_id, new_id, "created が違えば entry_id は異なる");

    cache.ingest_envelope(&envelope_from_core(&old, signer.as_ref()), Some(&old_id)).unwrap();
    cache.ingest_envelope(&envelope_from_core(&new, signer.as_ref()), Some(&new_id)).unwrap();
    assert_eq!(cache.size(), 2, "両版が併存する");

    // 同一 question_key で束ねられている。
    let qk_old = cache.get(&old_id).unwrap().question_key.clone();
    let qk_new = cache.get(&new_id).unwrap().question_key.clone();
    assert_eq!(qk_old, qk_new, "同一質問は同一 question_key");

    // 検索は両方を候補にし、同点なら created の新しい版を返す(§4 消費側選好)。
    let r = cache.lookup("日本の首都はどこですか");
    let hit = r.entry.expect("ヒットするはず");
    assert_eq!(hit.entry_id, new_id, "同点時は created の新しい版を選ぶ");
}

#[test]
fn merge_is_commutative_and_idempotent()
{
    // 2版を順序 [old,new] と [new,old] で取り込んでも同じ集合に収束する。
    let old = capital_core(&rfc3339_days_ago(10));
    let new = capital_core(&rfc3339_days_ago(1));
    let old_id = entry_id(&encode_core(&old));
    let new_id = entry_id(&encode_core(&new));

    let (mut c1, s1, _d1) = fresh_cache("merge_ord1");
    c1.ingest_envelope(&envelope_from_core(&old, s1.as_ref()), Some(&old_id)).unwrap();
    c1.ingest_envelope(&envelope_from_core(&new, s1.as_ref()), Some(&new_id)).unwrap();
    // 重複再投入(冪等)。
    c1.ingest_envelope(&envelope_from_core(&old, s1.as_ref()), Some(&old_id)).unwrap();

    let (mut c2, s2, _d2) = fresh_cache("merge_ord2");
    c2.ingest_envelope(&envelope_from_core(&new, s2.as_ref()), Some(&new_id)).unwrap();
    c2.ingest_envelope(&envelope_from_core(&old, s2.as_ref()), Some(&old_id)).unwrap();

    assert_eq!(c1.size(), 2);
    assert_eq!(c2.size(), 2);
    assert!(c1.contains(&old_id) && c1.contains(&new_id));
    assert!(c2.contains(&old_id) && c2.contains(&new_id));
}

// ------------------------------------------------------------------
// IngestError の各系統(改ざん・破損の検知)
// ------------------------------------------------------------------

#[test]
fn error_unsupported_schema()
{
    let (mut cache, signer, _dir) = fresh_cache("err_schema");
    let core = capital_core(&rfc3339_days_ago(0));
    let mut env = envelope_from_core(&core, signer.as_ref());
    env.schema_ver = 99;
    let err = cache.ingest_envelope(&env, None).unwrap_err();
    assert_eq!(err, IngestError::UnsupportedSchema(99));
}

#[test]
fn error_base64_decode()
{
    let (mut cache, signer, _dir) = fresh_cache("err_b64");
    let core = capital_core(&rfc3339_days_ago(0));
    let mut env = envelope_from_core(&core, signer.as_ref());
    env.core_b64 = "!!! not base64 !!!".to_string();
    let err = cache.ingest_envelope(&env, None).unwrap_err();
    assert_eq!(err, IngestError::Base64Decode);
}

#[test]
fn error_hash_mismatch_on_tampered_core()
{
    // core バイト列を1バイト改変 → 期待 entry_id と不一致(改ざん検知)。
    let (mut cache, signer, _dir) = fresh_cache("err_hash");
    let core = capital_core(&rfc3339_days_ago(0));
    let orig_id = entry_id(&encode_core(&core));
    let env = envelope_from_core(&core, signer.as_ref());

    let mut bytes = B64.decode(&env.core_b64).unwrap();
    bytes[0] ^= 0x01; // 1バイト改変(ドメインタグ先頭)
    let tampered = EntryEnvelope
    {
        schema_ver: env.schema_ver,
        core_b64: B64.encode(&bytes),
        author_pub: env.author_pub.clone(),
        author_sig: env.author_sig.clone(),
    };
    // 期待 entry_id を渡すと、署名検証より前のハッシュ照合で弾かれる。
    let err = cache.ingest_envelope(&tampered, Some(&orig_id)).unwrap_err();
    assert!(matches!(err, IngestError::HashMismatch { .. }), "改ざんは HashMismatch: {err:?}");
}

#[test]
fn error_bad_signature()
{
    // author_sig を改変 → 署名検証失敗(偽造防止)。
    let (mut cache, signer, _dir) = fresh_cache("err_sig");
    let core = capital_core(&rfc3339_days_ago(0));
    let id = entry_id(&encode_core(&core));
    let mut env = envelope_from_core(&core, signer.as_ref());

    // 先頭 hex 文字を別の値へ(有効 hex を保ったまま署名を壊す)。
    let mut chars: Vec<char> = env.author_sig.chars().collect();
    chars[0] = if chars[0] == 'a' { 'b' } else { 'a' };
    env.author_sig = chars.into_iter().collect();

    let err = cache.ingest_envelope(&env, Some(&id)).unwrap_err();
    assert_eq!(err, IngestError::BadSignature);
}

#[test]
fn error_malformed_core()
{
    // ハッシュ・署名は正しいが core の中身が形式不正(domain tag 不一致)。
    let (mut cache, signer, _dir) = fresh_cache("err_malformed");
    let bogus = b"this is definitely not a valid canonical core".to_vec();
    let id = entry_id(&bogus);
    let env = EntryEnvelope
    {
        schema_ver: crate::entry::SCHEMA_VER,
        core_b64: B64.encode(&bogus),
        author_pub: signer.public_key_hex().to_string(),
        author_sig: signer.sign_bytes(&bogus),
    };
    let err = cache.ingest_envelope(&env, Some(&id)).unwrap_err();
    assert_eq!(err, IngestError::MalformedCore);
}
