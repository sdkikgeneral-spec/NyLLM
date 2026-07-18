// H-1【CRL遡及除外】の回帰テスト(脅威レビュー指摘 H-1 / S3設計ノート §7)。
//
// 修正の骨子(実装本体 policy.rs / sync.rs):
//   CRL は ingest 時の門(verify_author=手順2)だけでなく、既に company_store に
//   取り込み済みのエントリにも遡及的に効かなければならない。著者ノードが失効
//   したら、そのノードが過去に作ったエントリを:
//     (a) 検索(ask/lookup_filtered → is_searchable)からヒットさせない
//     (b) 他ノードへ供出(handle_entry_request)しない
//     (c) Digest 列挙(handle_digest_request)から外す
//   ただし物理削除はしない(grow-only 維持)ため:
//     (d) .entry ファイルはディスクに残る
//     (e) CRL から外せば(失効解除)復活する = 削除ではなくフィルタ
//   さらに is_author_revoked は author_pub から node_id を直接再計算するので、
//   cert 表に該当 cert が未登録でも CRL 照合が成立する。
//
// DummySigner / ed25519 いずれのビルドでも、CRL に載せる node_id は
// issue_node_cert が author_pub から導出した cert.node_id と一致する
// (= node_id(author_pub))。common.rs の a.cert.node_id をそのまま使う。

use super::common::{
    build_company_network, future_expiry, new_ca, new_signer, temp_dir, SHAREABLE_QUESTION,
};
use crate::node::{issue_node_cert, node_id, Crl, Mode};
use crate::policy::{CertPolicy, CompanyCertPolicy};
use std::sync::Arc;

// ------------------------------------------------------------------
// (a)〜(e) 統合: 取込済みエントリの遡及除外と失効解除での復活
// ------------------------------------------------------------------

#[test]
fn revoked_author_entry_is_retroactively_excluded_and_restorable()
{
    let net = crate::transport::InMemoryNetwork::new();
    let dir = temp_dir("h1_retroactive");
    let ca = new_ca(&dir);
    // 1ノード(A)で、A が自ら登録したエントリを A 自身の失効で遡及除外する。
    let nodes = build_company_network(&net, &dir, &["a"], &ca);
    let a = &nodes[0];

    // A が首都エントリを登録(共有可=shareable=true)。
    let res = a.svc.ask(SHAREABLE_QUESTION);
    assert!(!res.hit, "初回はミス(新規登録)");
    assert!(res.shareable, "首都エントリは共有可");
    let eid = res.entry_id.clone();

    // 失効前のベースライン: 検索ヒット・供出可・Digest掲載。
    assert!(a.svc.ask(SHAREABLE_QUESTION).hit, "(a前提) 失効前は検索でヒット");
    assert!(
        a.svc.handle_entry_request(&eid).is_some(),
        "(b前提) 失効前は Transfer 供出できる"
    );
    assert!(
        a.svc.handle_digest_request().entries.iter().any(|i| i.entry_id == eid),
        "(c前提) 失効前は Digest に掲載される"
    );

    // 失効: A の node_id(=author の node_id)を CRL に載せる。
    a.cert_policy.set_crl(Crl { revoked: vec![a.cert.node_id.clone()] });

    // (a) 検索からヒットしなくなる(is_searchable 経由)。
    assert!(
        !a.svc.ask(SHAREABLE_QUESTION).hit,
        "(a) 失効著者のエントリは検索でヒットしない"
    );
    // (b) handle_entry_request が None を返す。
    assert!(
        a.svc.handle_entry_request(&eid).is_none(),
        "(b) 失効著者のエントリは供出しない"
    );
    // (c) Digest 列挙から消える。
    assert!(
        a.svc.handle_digest_request().entries.iter().all(|i| i.entry_id != eid),
        "(c) 失効著者のエントリは Digest から外れる"
    );

    // (d) 物理 .entry ファイルは残る(grow-only。物理削除しない)。
    let entry_file = dir.join("store_a").join(format!("{eid}.entry"));
    assert!(
        entry_file.exists(),
        "(d) 検索・供出除外でも .entry ファイルは残存する: {}",
        entry_file.display()
    );

    // (e) CRL 解除で復活する(削除ではなくフィルタであることの実証)。
    a.cert_policy.set_crl(Crl::default());
    assert!(
        a.svc.handle_entry_request(&eid).is_some(),
        "(e) 失効解除で供出が復活する = フィルタであって削除ではない"
    );
    assert!(
        a.svc.handle_digest_request().entries.iter().any(|i| i.entry_id == eid),
        "(e) 失効解除で Digest 掲載も復活する"
    );
    assert!(a.svc.ask(SHAREABLE_QUESTION).hit, "(e) 失効解除で検索ヒットも復活する");
}

// ------------------------------------------------------------------
// is_author_revoked: cert 表未登録でも author_pub から node_id を再計算して照合
// ------------------------------------------------------------------

#[test]
fn is_author_revoked_recomputes_node_id_without_cert_entry()
{
    let dir = temp_dir("h1_no_cert");
    let ca = new_ca(&dir);
    let node = new_signer(&dir, "n1");
    let author_pub = node.public_key_hex().to_string();

    // cert を1件も upsert しない CompanyCertPolicy。
    let pol = CompanyCertPolicy::new(ca.clone(), ca.public_key_hex());

    // CRL 未設定 → 失効なし。
    assert!(
        !pol.is_author_revoked(&author_pub),
        "CRL 未設定なら失効していない"
    );

    // author_pub から直接再計算した node_id を CRL に載せる。
    // (cert 表には何も入れていない = cert 経由の照合には依存しないことの実証)
    pol.set_crl(Crl { revoked: vec![node_id(&author_pub)] });
    assert!(
        pol.is_author_revoked(&author_pub),
        "cert 未登録でも node_id 直接再計算で CRL 照合が成立する"
    );

    // 別の(失効していない)公開鍵は false のまま。
    // 注意: 既定(DummySigner)ビルドでは common.rs::key_path が全ノードに
    // shared.key を共有させるため、new_signer(&dir, "other") では n1 と同一の
    // 公開鍵(=同一 node_id)になり、このネガティブ検証が成立しない。
    // node_id() は公開鍵 hex に対して決定的なので、n1 と異なる固定の公開鍵 hex
    // を直接使えば、両ビルドで同じ意味の検証になる(署名可能な鍵である必要は
    // ない: is_author_revoked は node_id 再計算と CRL 照合しかしない)。
    let other_pub = "00000000000000000000000000000000000000000000000000000000000000ff";
    // 検証前提の厳密な形は「node_id 同士の相異」である(is_author_revoked の
    // CRL 照合単位は node_id であるため)。生の公開鍵 hex の相異は sha256 の
    // 衝突耐性を介した proxy にすぎないので、node_id を直接比較して保証する。
    assert_ne!(
        node_id(other_pub),
        node_id(&author_pub),
        "検証前提: n1 とは異なる node_id であること(CRL 照合単位での相異)"
    );
    assert!(
        !pol.is_author_revoked(other_pub),
        "CRL に載っていない著者は失効扱いにならない"
    );
}

// ------------------------------------------------------------------
// cert 経由の node_id と author_pub 直接再計算の node_id が一致すること
// (H-1 が cert 表の有無に関わらず同じ node_id で照合できる根拠)
// ------------------------------------------------------------------

#[test]
fn cert_node_id_equals_recomputed_node_id()
{
    let dir = temp_dir("h1_nodeid_consistency");
    let ca = new_ca(&dir);
    let node = new_signer(&dir, "n1");
    let cert = issue_node_cert(
        ca.as_ref(),
        node.public_key_hex(),
        &future_expiry(),
        &[Mode::Company],
    );
    assert_eq!(
        cert.node_id,
        node_id(node.public_key_hex()),
        "cert.node_id と author_pub からの再計算 node_id は一致する"
    );

    // Arc<dyn CertPolicy> 経由でも同一挙動(sync.rs が触る型での確認)。
    let pol: Arc<dyn CertPolicy> = Arc::new(CompanyCertPolicy::new(ca.clone(), ca.public_key_hex()));
    assert!(!pol.is_author_revoked(node.public_key_hex()));
}
