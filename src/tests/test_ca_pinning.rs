// M-1【信頼アンカー(CA公開鍵)のピン留め / TOFU】の回帰テスト
// (脅威レビュー指摘 M-1 / policy.rs CompanyCertPolicy::new / set_ca_pub)。
//
// 修正の骨子(実装本体 policy.rs):
//   レジストリは発見のみで信頼判断を持たない(§8-1)。CA公開鍵はローカル設定で
//   ピン留めするか、未設定時のみレジストリ供給で TOFU ブートストラップする。
//     - 非空 CA pub で構築 → ピン留め。以後 set_ca_pub(レジストリ供給)は常に無視。
//     - 空構築 → 未ピン。初回 set_ca_pub のみ反映し、以後は上書きしない(TOFU固定)。
//   これによりレジストリ compromise 一発で信頼アンカーを差し替えられる事態を防ぐ。
//
// 検証は挙動ベース(set_ca_pub の戻り値 = 反映されたか、および verify_author が
// どの CA で検証を継続するか)で行う。DummySigner / ed25519 いずれのビルドでも、
// 元CAで発行した cert は元CA公開鍵での検証に通り、CA を差し替えれば通らなくなる。

use super::common::{future_expiry, new_ca, new_signer, temp_dir};
use crate::node::{issue_node_cert, Mode};
use crate::policy::{CertPolicy, CompanyCertPolicy};

// 別秘密の CA を2つ用意する(new_ca は dir 単位で ca.key を作るので dir を分ける)。
fn two_cas() -> (std::sync::Arc<dyn crate::signer::Signer>, std::sync::Arc<dyn crate::signer::Signer>, std::path::PathBuf)
{
    let dir1 = temp_dir("m1_ca1");
    let dir2 = temp_dir("m1_ca2");
    let ca1 = new_ca(&dir1);
    let ca2 = new_ca(&dir2);
    (ca1, ca2, dir1)
}

// ------------------------------------------------------------------
// 非空構築 = ピン留め: set_ca_pub(別鍵)は無視され、元CAで検証継続
// ------------------------------------------------------------------

#[test]
fn pinned_ca_ignores_set_ca_pub()
{
    let (ca1, ca2, dir) = two_cas();
    let node = new_signer(&dir, "n1");
    let cert1 = issue_node_cert(ca1.as_ref(), node.public_key_hex(), &future_expiry(), &[Mode::Company]);

    // ローカル設定(非空 CA pub)で構築 = ピン留め。
    let pol = CompanyCertPolicy::new(ca1.clone(), ca1.public_key_hex());
    pol.upsert_cert(cert1);
    assert!(pol.verify_author(node.public_key_hex()).is_ok(), "元CAで検証は通る");

    // 別鍵(ca2)で set_ca_pub → ピン留め済みなので無視される(戻り値 false)。
    assert!(
        !pol.set_ca_pub(ca2.public_key_hex()),
        "ピン留め済みノードは set_ca_pub(レジストリ供給)を無視する"
    );
    // 元CA(ca1)で検証が継続する(信頼アンカーは差し替わっていない)。
    assert!(
        pol.verify_author(node.public_key_hex()).is_ok(),
        "set_ca_pub 無視後も元CAで検証継続"
    );
}

// ------------------------------------------------------------------
// 別CA発行の cert は元CA(ピン留め)では検証に通らない(アンカー差替が起きていない証左)
// ------------------------------------------------------------------

#[test]
fn pinned_ca_rejects_cert_from_other_ca()
{
    let (ca1, ca2, dir) = two_cas();
    let node = new_signer(&dir, "n1");
    // 別CA(ca2)が発行した cert。
    let cert2 = issue_node_cert(ca2.as_ref(), node.public_key_hex(), &future_expiry(), &[Mode::Company]);

    let pol = CompanyCertPolicy::new(ca1.clone(), ca1.public_key_hex()); // ca1 にピン留め
    pol.upsert_cert(cert2);
    // set_ca_pub(ca2) が無視される以上、ca2 発行 cert は元CA(ca1)検証で落ちる。
    assert!(!pol.set_ca_pub(ca2.public_key_hex()));
    assert!(
        pol.verify_author(node.public_key_hex()).is_err(),
        "別CA発行の cert はピン留めされた元CAでは検証失敗する"
    );
}

// ------------------------------------------------------------------
// 空構築 = 未ピン: 初回 set_ca_pub のみ反映、2回目以降は無視(TOFU)
// ------------------------------------------------------------------

#[test]
fn empty_ca_bootstraps_once_then_tofu_locks()
{
    let (ca1, ca2, dir) = two_cas();
    let node = new_signer(&dir, "n1");
    let cert1 = issue_node_cert(ca1.as_ref(), node.public_key_hex(), &future_expiry(), &[Mode::Company]);

    // 空文字で構築 = 未ピン(レジストリ供給でブートストラップ可能な状態)。
    let pol = CompanyCertPolicy::new(ca1.clone(), "");
    pol.upsert_cert(cert1);

    // ブートストラップ前: CA未設定なので検証不可。
    assert!(
        pol.verify_author(node.public_key_hex()).is_err(),
        "CA公開鍵未設定では検証できない"
    );
    // 空文字の set_ca_pub は反映しない(戻り値 false)。
    assert!(!pol.set_ca_pub(""), "空の CA pub 供給は無視する");

    // 初回 set_ca_pub(ca1)は反映される(TOFU ブートストラップ)。
    assert!(pol.set_ca_pub(ca1.public_key_hex()), "初回供給は反映される(TOFU)");
    assert!(
        pol.verify_author(node.public_key_hex()).is_ok(),
        "ブートストラップ後は元CAで検証できる"
    );

    // 2回目以降(別鍵 ca2)は無視される(一度入った値は上書きしない = TOFU固定)。
    assert!(
        !pol.set_ca_pub(ca2.public_key_hex()),
        "ブートストラップ済みなら以後の供給値は無視する(TOFU固定)"
    );
    assert!(
        pol.verify_author(node.public_key_hex()).is_ok(),
        "TOFU固定後も初回CAで検証継続(レジストリ compromise でアンカー差替できない)"
    );
}
