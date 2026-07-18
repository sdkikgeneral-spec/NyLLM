// node.rs / policy.rs(組織PKI)の単体テスト(S3設計ノート §1「ノード・信頼モデル」)。
//
// 検証観点:
//   - node_id のドメイン分離(nyllm/node/v1 タグ)と決定性
//   - Mode パース/表示
//   - node_cert 発行→検証(CA署名・node_id整合・有効期限・mode許可)
//   - CompanyCertPolicy の verify_author(未登録/失効/期限切れ/mode不許可)
//   - CRL 失効判定

use super::common::{future_expiry, new_ca, new_signer, past_expiry, temp_dir};
use crate::node::{
    cert_allows_mode, issue_node_cert, node_id, verify_node_cert, Crl, Mode, NODE_DOMAIN_TAG,
};
use crate::policy::{CertPolicy, CompanyCertPolicy, RejectAllCertPolicy};
use crate::entry::sha256_hex;
use chrono::Utc;
use std::sync::Arc;

#[test]
fn node_id_is_deterministic_and_domain_tagged()
{
    // ある公開鍵(hex)に対する node_id は決定的。
    let pubhex = "aabbccdd";
    let a = node_id(pubhex);
    let b = node_id(pubhex);
    assert_eq!(a, b, "node_id は決定的でなければならない");

    // 生の sha256(pub) とは異なる(ドメインタグ nyllm/node/v1 が前置されるため)。
    let raw = sha256_hex(&hex::decode(pubhex).unwrap());
    assert_ne!(a, raw, "node_id はドメインタグ付きで生ハッシュと異なるべき");

    // タグ + pub_bytes の sha256 に一致する(仕様の明示ピン留め)。
    let mut buf = NODE_DOMAIN_TAG.to_vec();
    buf.extend_from_slice(&hex::decode(pubhex).unwrap());
    assert_eq!(a, sha256_hex(&buf));
}

#[test]
fn mode_parse_and_as_str()
{
    assert_eq!(Mode::parse("company"), Some(Mode::Company));
    assert_eq!(Mode::parse("private"), Some(Mode::Private));
    assert_eq!(Mode::parse("public"), None);
    assert_eq!(Mode::Company.as_str(), "company");
    assert_eq!(Mode::Private.as_str(), "private");
}

#[test]
fn issue_and_verify_node_cert_ok()
{
    let dir = temp_dir("node_cert_ok");
    let ca = new_ca(&dir);
    let node = new_signer(&dir, "n1");
    let cert = issue_node_cert(ca.as_ref(), node.public_key_hex(), &future_expiry(), &[Mode::Company]);

    // 正規の CA 公開鍵で検証が通る。
    assert!(verify_node_cert(ca.as_ref(), ca.public_key_hex(), &cert, Utc::now()).is_ok());
    // company 許可を含む。
    assert!(cert_allows_mode(&cert, Mode::Company));
    assert!(!cert_allows_mode(&cert, Mode::Private));
}

#[test]
fn verify_node_cert_rejects_wrong_ca()
{
    let dir = temp_dir("node_cert_wrongca");
    let ca1 = new_ca(&dir);
    // 別 CA(別秘密)。dir を変えて別鍵にする。
    let dir2 = temp_dir("node_cert_wrongca2");
    let ca2 = new_ca(&dir2);
    let node = new_signer(&dir, "n1");

    let cert = issue_node_cert(ca1.as_ref(), node.public_key_hex(), &future_expiry(), &[Mode::Company]);
    // ca2 の公開鍵/検証器で検証 → CA署名不一致で失敗。
    let r = verify_node_cert(ca2.as_ref(), ca2.public_key_hex(), &cert, Utc::now());
    assert!(r.is_err(), "別CAで発行された cert は検証失敗すべき");
}

#[test]
fn verify_node_cert_rejects_expired()
{
    let dir = temp_dir("node_cert_expired");
    let ca = new_ca(&dir);
    let node = new_signer(&dir, "n1");
    let cert = issue_node_cert(ca.as_ref(), node.public_key_hex(), &past_expiry(), &[Mode::Company]);
    let r = verify_node_cert(ca.as_ref(), ca.public_key_hex(), &cert, Utc::now());
    assert!(r.is_err(), "期限切れ cert は検証失敗すべき");
}

#[test]
fn verify_node_cert_rejects_at_exact_expiry_boundary()
{
    // §10観点6の境界網: node.rs の期限判定は `expires <= now` であり、
    // 境界時刻(expires == now ちょうど)は「期限切れ」側に倒す保守的判定。
    // now を外から渡せる verify_node_cert の性質を使い、秒精度で完全一致させる。
    let dir = temp_dir("node_cert_boundary");
    let ca = new_ca(&dir);
    let node = new_signer(&dir, "n1");

    // expires 文字列を先に確定し、それをパースし直した値を now とする
    // (フォーマット往復による丸め差を排除して expires == now を厳密に作る)。
    let expires = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let now = chrono::DateTime::parse_from_rfc3339(&expires)
        .unwrap()
        .with_timezone(&Utc);
    let cert = issue_node_cert(ca.as_ref(), node.public_key_hex(), &expires, &[Mode::Company]);

    // 境界ちょうどは失効扱い(保守側)。
    let r = verify_node_cert(ca.as_ref(), ca.public_key_hex(), &cert, now);
    assert!(r.is_err(), "expires == now の境界時刻は期限切れとして拒否すべき(保守側)");

    // 対照: 境界の1秒前なら有効 = 失効に倒れるのは境界以降だけであることの裏付け。
    let just_before = now - chrono::Duration::seconds(1);
    assert!(
        verify_node_cert(ca.as_ref(), ca.public_key_hex(), &cert, just_before).is_ok(),
        "expires 直前(1秒前)は有効(境界のみが失効側に倒れることの対照)"
    );
}

#[test]
fn verify_node_cert_rejects_id_tamper()
{
    let dir = temp_dir("node_cert_idtamper");
    let ca = new_ca(&dir);
    let node = new_signer(&dir, "n1");
    let mut cert = issue_node_cert(ca.as_ref(), node.public_key_hex(), &future_expiry(), &[Mode::Company]);
    // node_id を差し替える(pub と不整合)→ ID詐称として弾かれる。
    cert.node_id = "0000000000000000".to_string();
    let r = verify_node_cert(ca.as_ref(), ca.public_key_hex(), &cert, Utc::now());
    assert!(r.is_err(), "node_id を差し替えた cert は検証失敗すべき");
}

#[test]
fn crl_is_revoked()
{
    let crl = Crl { revoked: vec!["abc".to_string(), "def".to_string()] };
    assert!(crl.is_revoked("abc"));
    assert!(!crl.is_revoked("zzz"));
    assert!(!Crl::default().is_revoked("abc"));
}

#[test]
fn company_cert_policy_accepts_valid_author()
{
    let dir = temp_dir("certpol_ok");
    let ca = new_ca(&dir);
    let node = new_signer(&dir, "n1");
    let cert = issue_node_cert(ca.as_ref(), node.public_key_hex(), &future_expiry(), &[Mode::Company]);

    let pol = CompanyCertPolicy::new(ca.clone(), ca.public_key_hex());
    pol.upsert_cert(cert);
    assert!(pol.verify_author(node.public_key_hex()).is_ok());
}

#[test]
fn company_cert_policy_rejects_unregistered_author()
{
    let dir = temp_dir("certpol_unreg");
    let ca = new_ca(&dir);
    let pol = CompanyCertPolicy::new(ca.clone(), ca.public_key_hex());
    // cert を1件も入れていない → 未登録 pub は拒否。
    let r = pol.verify_author("deadbeefdeadbeefdeadbeefdeadbeef");
    assert!(r.is_err(), "cert 未登録の author は拒否すべき");
}

#[test]
fn company_cert_policy_rejects_revoked_via_crl()
{
    let dir = temp_dir("certpol_crl");
    let ca = new_ca(&dir);
    let node = new_signer(&dir, "n1");
    let cert = issue_node_cert(ca.as_ref(), node.public_key_hex(), &future_expiry(), &[Mode::Company]);

    let pol = CompanyCertPolicy::new(ca.clone(), ca.public_key_hex());
    pol.upsert_cert(cert.clone());
    assert!(pol.verify_author(node.public_key_hex()).is_ok(), "失効前は通る");

    // cert.node_id を CRL に載せる → 以後は拒否。
    pol.set_crl(Crl { revoked: vec![cert.node_id.clone()] });
    assert!(pol.verify_author(node.public_key_hex()).is_err(), "CRL 失効後は拒否すべき");
}

#[test]
fn company_cert_policy_rejects_expired_cert()
{
    let dir = temp_dir("certpol_expired");
    let ca = new_ca(&dir);
    let node = new_signer(&dir, "n1");
    let cert = issue_node_cert(ca.as_ref(), node.public_key_hex(), &past_expiry(), &[Mode::Company]);
    let pol = CompanyCertPolicy::new(ca.clone(), ca.public_key_hex());
    pol.upsert_cert(cert);
    assert!(pol.verify_author(node.public_key_hex()).is_err(), "期限切れ cert の author は拒否すべき");
}

#[test]
fn company_cert_policy_rejects_mode_not_allowed()
{
    let dir = temp_dir("certpol_mode");
    let ca = new_ca(&dir);
    let node = new_signer(&dir, "n1");
    // private のみ許可の cert → company 配送の verify_author は拒否。
    let cert = issue_node_cert(ca.as_ref(), node.public_key_hex(), &future_expiry(), &[Mode::Private]);
    let pol = CompanyCertPolicy::new(ca.clone(), ca.public_key_hex());
    pol.upsert_cert(cert);
    assert!(pol.verify_author(node.public_key_hex()).is_err(), "company 未許可の cert は拒否すべき");
}

#[test]
fn reject_all_policy_always_errors()
{
    let pol = RejectAllCertPolicy;
    assert!(pol.verify_author("anything").is_err());
    // 型が Arc<dyn CertPolicy> として使えることも確認(private への配線)。
    let dyn_pol: Arc<dyn CertPolicy> = Arc::new(RejectAllCertPolicy);
    assert!(dyn_pol.verify_author("x").is_err());
}
