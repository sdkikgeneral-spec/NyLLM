// 署名層のテスト(signer.rs / DummySigner を Signer トレイト経由で使用)。
//
// S2.5(docs/S2.5_エントリ形式設計.md §5, §10-5):
//   - 署名対象は immutable_core の正準バイト列(core_bytes = バイナリ)そのもの。
//     trait は sign_hex(&str) から sign_bytes(&[u8]) / verify(.., &[u8]) へ変更された。
//     本ファイルは全テストを &[u8] API へ移行する。
//   - DummySigner は旧 sha256(secret || payload) が長さ拡張攻撃を許すため
//     HMAC-SHA256(RFC 2104)へ変更された(§5 付随修正)。それを既知ベクトルで
//     ピン留めし、同時に旧構成(secret||data の素の SHA-256)と異なることを確認する。
//
// DummySigner は鍵付き MAC。検証には秘密鍵が要る(公開検証不可)ため、verify() は
// 自ノード鍵(pub_hex 一致)でしか成功しない。ここではそのラウンドトリップと限界、
// 鍵ファイルの永続化・再読込・親ディレクトリ自動生成、HMAC 構成を確認する。
//
// これらのテスト(ed25519 のもの以外)は DummySigner を直接使うため
// feature="ed25519" には依存しない。

use crate::signer::{DummySigner, Signer};
use sha2::{Digest, Sha256};

#[test]
fn sign_then_verify_roundtrip_succeeds()
{
    let dir = super::common::temp_dir("signer_roundtrip");
    let keyfile = dir.join("node.key");

    let signer = DummySigner::new(&keyfile).expect("signer 初期化に失敗");
    // core_bytes を模した任意バイナリ(NUL や非UTF-8 バイトを含んでよい)。
    let payload: &[u8] = b"canonical-\x00\x01\xffpayload";
    let sig = signer.sign_bytes(payload);

    assert!(
        signer.verify(signer.public_key_hex(), &sig, payload),
        "自ノード鍵での署名検証が失敗した"
    );
}

#[test]
fn verify_fails_for_different_payload()
{
    let dir = super::common::temp_dir("signer_diff_payload");
    let keyfile = dir.join("node.key");

    let signer = DummySigner::new(&keyfile).expect("signer 初期化に失敗");
    let sig = signer.sign_bytes(b"original-payload");

    assert!(
        !signer.verify(signer.public_key_hex(), &sig, b"different-payload"),
        "異なるバイト列で検証が成功してしまった"
    );
}

#[test]
fn verify_fails_for_tampered_signature()
{
    let dir = super::common::temp_dir("signer_tampered_sig");
    let keyfile = dir.join("node.key");

    let signer = DummySigner::new(&keyfile).expect("signer 初期化に失敗");
    let payload: &[u8] = b"canonical-payload";
    let sig = signer.sign_bytes(payload);

    // 署名文字列の先頭 1 文字を別の 16 進文字に変える。
    let mut tampered = sig.clone();
    let replacement = if tampered.starts_with('0') { "f" } else { "0" };
    tampered.replace_range(0..1, replacement);
    assert_ne!(tampered, sig);

    assert!(
        !signer.verify(signer.public_key_hex(), &tampered, payload),
        "改ざんされた署名で検証が成功してしまった"
    );
}

#[test]
fn verify_fails_across_different_keys()
{
    // 別の鍵ファイルパスで生成した別 DummySigner(=別公開鍵)で、
    // 最初の署名者の author_pub + sig を検証 → 失敗。
    // verify() は pub_hex 不一致で即 false(MAC の限界の実証)。
    let dir = super::common::temp_dir("signer_two_keys");
    let key_a = dir.join("a.key");
    let key_b = dir.join("b.key");

    let signer_a = DummySigner::new(&key_a).expect("signer A 初期化に失敗");
    let signer_b = DummySigner::new(&key_b).expect("signer B 初期化に失敗");

    // 別々の秘密鍵なので公開鍵も異なるはず。
    assert_ne!(
        signer_a.public_key_hex(),
        signer_b.public_key_hex(),
        "別鍵なのに公開鍵が一致した"
    );

    let payload: &[u8] = b"canonical-payload";
    let sig_a = signer_a.sign_bytes(payload);

    // B に対して A の公開鍵 + 署名を渡しても、B は A の鍵では検証できない。
    assert!(
        !signer_b.verify(signer_a.public_key_hex(), &sig_a, payload),
        "他ノード鍵の署名を検証できてしまった(MAC の限界に反する)"
    );
}

#[test]
fn same_key_file_reloads_and_verifies()
{
    // 同じ鍵ファイルパスで 2 つ目の DummySigner を作ると公開鍵が一致し、
    // 一方の署名がもう一方の verify() でも成功する(鍵の永続化・再読込)。
    let dir = super::common::temp_dir("signer_reload");
    let keyfile = dir.join("node.key");

    let signer1 = DummySigner::new(&keyfile).expect("signer1 初期化に失敗");
    let signer2 = DummySigner::new(&keyfile).expect("signer2 初期化に失敗");

    assert_eq!(
        signer1.public_key_hex(),
        signer2.public_key_hex(),
        "同一鍵ファイルから再読込したのに公開鍵が一致しない"
    );

    let payload: &[u8] = b"canonical-payload";
    let sig1 = signer1.sign_bytes(payload);

    assert!(
        signer2.verify(signer1.public_key_hex(), &sig1, payload),
        "同一鍵で作った別インスタンスが署名を検証できなかった"
    );
}

#[test]
fn creates_missing_multi_level_parent_dirs()
{
    // 存在しない多階層の親ディレクトリを持つ鍵パスでも new が成功し、
    // 鍵ファイルが実際に作られる(内部で create_dir_all される)。
    let dir = super::common::temp_dir("signer_deep");
    let keyfile = dir.join("a").join("b").join("c").join("node.key");
    assert!(!keyfile.exists(), "前提: 鍵ファイルはまだ存在しないはず");

    let signer = DummySigner::new(&keyfile).expect("多階層親ディレクトリでの初期化に失敗");
    assert!(keyfile.exists(), "鍵ファイルが作成されなかった");
    assert!(!signer.public_key_hex().is_empty(), "公開鍵が空");
}

#[test]
fn dummy_signer_is_hmac_sha256_not_bare_concat()
{
    // DummySigner の MAC が HMAC-SHA256 であることを既知ベクトルでピン留めする。
    // 併せて旧構成 sha256(secret || data)(長さ拡張攻撃が成立しうる。§5 弱点6)と
    // 出力が一致しないことを確認し、HMAC 化(= 長さ拡張耐性)を担保する。
    //
    // 手法: DummySigner::new は鍵ファイルが既存ならその内容を秘密鍵として読む。
    // そこで RFC 4231 Test Case 2 の鍵 "Jefe" を鍵ファイルに書き込んでから
    // 署名させ、sign_bytes(data) が RFC 4231 の HMAC-SHA256 期待値に一致するか
    // を確認する。
    let dir = super::common::temp_dir("signer_hmac_kat");
    let keyfile = dir.join("node.key");
    // RFC 4231 §4.3 Test Case 2 の鍵をそのまま秘密鍵として使う。
    std::fs::write(&keyfile, b"Jefe").expect("鍵ファイルの書き込みに失敗");

    let signer = DummySigner::new(&keyfile).expect("signer 初期化に失敗");
    let data = b"what do ya want for nothing?";

    // RFC 4231 Test Case 2 の HMAC-SHA256 期待値。
    const RFC4231_TC2: &str =
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843";
    let sig = signer.sign_bytes(data);
    assert_eq!(
        sig, RFC4231_TC2,
        "DummySigner の MAC が HMAC-SHA256 の既知ベクトルと一致しない"
    );

    // 旧構成 sha256(secret || data) を独立計算し、HMAC 出力と異なることを確認。
    // (長さ拡張攻撃が成立する素の連結ハッシュではないことの証明)
    let mut naive = Sha256::new();
    naive.update(b"Jefe");
    naive.update(data);
    let naive_hex = hex::encode(naive.finalize());
    assert_ne!(
        sig, naive_hex,
        "MAC が sha256(secret||data) と一致した(長さ拡張脆弱な旧構成のまま)"
    );
}

#[cfg(feature = "ed25519")]
#[test]
fn ed25519_sign_bytes_verify_roundtrip()
{
    // feature="ed25519" 時: 実 Ed25519 でも core_bytes を模した &[u8] への
    // sign_bytes → verify が成立し、別データでは失敗する(§9 Ed25519 観点)。
    use crate::signer::Ed25519Signer;

    let dir = super::common::temp_dir("signer_ed25519");
    let keyfile = dir.join("node.key");
    let signer = Ed25519Signer::new(&keyfile).expect("ed25519 signer 初期化に失敗");

    // 非UTF-8 バイトを含む「正準バイト列」を模したデータ。
    let data: &[u8] = b"nyllm/entry/v1\n\x00\x01\x02core-bytes";
    let sig = signer.sign_bytes(data);
    assert!(
        signer.verify(signer.public_key_hex(), &sig, data),
        "Ed25519 の sign_bytes → verify ラウンドトリップが失敗した"
    );
    assert!(
        !signer.verify(signer.public_key_hex(), &sig, b"tampered-core-bytes"),
        "Ed25519 が異なるバイト列を検証してしまった"
    );
}
