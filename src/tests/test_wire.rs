// wire.rs(nyllm-wire/v1)の単体テスト(S3設計ノート §3「メッセージ型」/
// §8「配送抽象のバージョニング」)。
//
// 検証観点:
//   - 各メッセージ型の encode → decode ラウンドトリップ
//   - バージョンタグ不一致は Err(受信側が無視できる)
//   - 未知バリアント(Phase2 の FindNode 等の前借り)は Err(前方互換の最小形)
//   - digest_hash の決定性(同一集合 → 同一ハッシュ、差分 → 異なるハッシュ)

use crate::wire::{
    decode_message, digest_hash, encode_message, Announce, Digest, DigestItem, Request, Transfer,
    WireMessage, WIRE_VERSION,
};
use crate::entry::EntryEnvelope;

fn sample_envelope() -> EntryEnvelope
{
    EntryEnvelope
    {
        schema_ver: 1,
        core_b64: "AAAA".to_string(),
        author_pub: "pub".to_string(),
        author_sig: "sig".to_string(),
    }
}

#[test]
fn announce_roundtrip()
{
    let msg = WireMessage::Announce(Announce
    {
        entry_id: "id1".to_string(),
        question_key: "qk1".to_string(),
        created: "2026-07-18T00:00:00Z".to_string(),
        node_id: "nodeA".to_string(),
    });
    let s = encode_message(&msg);
    assert!(s.contains(WIRE_VERSION), "wire バージョンタグが載る");
    assert_eq!(decode_message(&s).unwrap(), msg);
}

#[test]
fn request_transfer_digest_roundtrip()
{
    let req = WireMessage::Request(Request { entry_id: "id1".to_string() });
    assert_eq!(decode_message(&encode_message(&req)).unwrap(), req);

    let tr = WireMessage::Transfer(Transfer { envelope: sample_envelope() });
    assert_eq!(decode_message(&encode_message(&tr)).unwrap(), tr);

    let dg = WireMessage::Digest(Digest
    {
        digest_hash: "h".to_string(),
        entries: vec![DigestItem { entry_id: "id1".to_string(), question_key: "qk1".to_string() }],
    });
    assert_eq!(decode_message(&encode_message(&dg)).unwrap(), dg);
}

#[test]
fn decode_rejects_wrong_version()
{
    // wire バージョンを差し替えた JSON は Err(受信側は無視できる)。
    let json = r#"{"wire":"nyllm-wire/v99","msg":{"type":"Request","entry_id":"x"}}"#;
    assert!(decode_message(json).is_err(), "非対応バージョンは Err");
}

#[test]
fn decode_rejects_unknown_variant()
{
    // Phase2 で追加される想定の未知バリアント → 現行 v1 ではデシリアライズ失敗。
    let json = r#"{"wire":"nyllm-wire/v1","msg":{"type":"FindNode","target":"x"}}"#;
    assert!(decode_message(json).is_err(), "未知バリアントは Err(前方互換の最小形)");
}

#[test]
fn digest_hash_is_deterministic_and_order_sensitive_input()
{
    // 同一(ソート済み)入力 → 同一ハッシュ。
    let a = digest_hash(&["id1".to_string(), "id2".to_string()]);
    let b = digest_hash(&["id1".to_string(), "id2".to_string()]);
    assert_eq!(a, b);

    // 集合内容が変われば別ハッシュ。
    let c = digest_hash(&["id1".to_string(), "id3".to_string()]);
    assert_ne!(a, c);

    // 空集合のハッシュも決定的。
    assert_eq!(digest_hash(&[]), digest_hash(&[]));
}
