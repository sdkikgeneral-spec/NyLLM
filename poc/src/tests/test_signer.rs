// 署名層のテスト(signer.rs / DummySigner を Signer トレイト経由で使用)。
//
// DummySigner は sha256(secret || payload) の鍵付き MAC。
// 検証には秘密鍵が要る(公開検証不可)ため、verify() は自ノード鍵
// (pub_hex 一致)でしか成功しない。ここではそのラウンドトリップと限界、
// および鍵ファイルの永続化・再読込・親ディレクトリ自動生成を確認する。
//
// これらのテストは DummySigner を直接使うため feature="ed25519" には依存しない。

use crate::signer::{DummySigner, Signer};

#[test]
fn sign_then_verify_roundtrip_succeeds()
{
    let dir = super::common::temp_dir("signer_roundtrip");
    let keyfile = dir.join("node.key");

    let signer = DummySigner::new(&keyfile).expect("signer 初期化に失敗");
    let payload = "canonical-payload";
    let sig = signer.sign_hex(payload);

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
    let sig = signer.sign_hex("original-payload");

    assert!(
        !signer.verify(signer.public_key_hex(), &sig, "different-payload"),
        "異なるペイロードで検証が成功してしまった"
    );
}

#[test]
fn verify_fails_for_tampered_signature()
{
    let dir = super::common::temp_dir("signer_tampered_sig");
    let keyfile = dir.join("node.key");

    let signer = DummySigner::new(&keyfile).expect("signer 初期化に失敗");
    let payload = "canonical-payload";
    let sig = signer.sign_hex(payload);

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

    let payload = "canonical-payload";
    let sig_a = signer_a.sign_hex(payload);

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

    let payload = "canonical-payload";
    let sig1 = signer1.sign_hex(payload);

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
