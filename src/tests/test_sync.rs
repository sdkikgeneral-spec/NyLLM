// マルチノード配送の結合テスト(S3設計ノート §10 テスト観点の本体)。
//
// プロセス内シミュレーション(InMemoryNetwork + InMemoryTransport。同期実行で
// 決定性)で NodeService を複数立て、announce→プル→検証→冪等マージの全経路を
// 1スレッドで通す。§10 の各観点をこのファイルで網羅する:
//   - 配送 round-trip / S3ゲート「2ノード以上で共有」
//   - 受信側再導出(ネット越し)
//   - anti-entropy(announce 損失の補償)
//   - author_sig/CA検証(CRL失効 → drop)
//   - 改ざん検知(ネット越し。core改変=HashMismatch / 署名改変=BadSignature)
//   - モード分離(private + delivery 拒否 / private ハンドラ不活性)
//   - TTL 検索除外(物理削除しない)
//   - ポリシー差替の非破壊性(AllowAll でも署名検証コア不変)
//   - 非共有エントリは供出・Digest 掲載されない

use super::common::{
    build_company_network, build_company_network_cfg, envelope_from_core, future_expiry,
    make_core, make_identity, new_ca, past_expiry, rfc3339_days_ago, shared_agent,
    shared_embedder, temp_dir, triple, SHAREABLE_QUESTION,
};
use crate::entry::{encode_core, entry_id, EntryEnvelope, Tier};
use crate::node::{issue_node_cert, Crl, Mode, NodeCert};
use crate::policy::{AllowAllCertPolicy, CompanyCertPolicy, PeerTable, Policies, RejectAllCertPolicy};
use crate::signer::Signer;
use crate::sync::{AnnounceOutcome, Delivery, NodeConfig, NodeService};
use crate::transport::InMemoryNetwork;
use crate::wire::{Announce, Transfer};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use std::sync::Arc;

// ------------------------------------------------------------------
// 配送 round-trip / S3ゲート
// ------------------------------------------------------------------

#[test]
fn round_trip_announce_pull_ingest()
{
    let net = InMemoryNetwork::new();
    let dir = temp_dir("roundtrip");
    let ca = new_ca(&dir);
    let nodes = build_company_network(&net, &dir, &["a", "b"], &ca);
    let (a, b) = (&nodes[0], &nodes[1]);

    let res = a.svc.ask(SHAREABLE_QUESTION);
    assert!(!res.hit, "初回はミス");
    assert!(res.shareable, "首都エントリは共有可");
    assert!(res.announced_to >= 1, "少なくとも1ピアへ announce 送達");

    // InMemory は同期実行 → ask 返却時点で B は取り込み済み。
    assert_eq!(a.svc.entry_count(), 1);
    assert_eq!(b.svc.entry_count(), 1, "B が配送で同一エントリを取得");
    assert!(
        b.svc.cache().lock().unwrap().contains(&res.entry_id),
        "B は同一 entry_id を保持(内容アドレス性)"
    );
}

#[test]
fn s3_gate_two_or_more_nodes_share()
{
    // S3ゲート:「2ノード以上で共有できる」ことをテストで実証する。
    let net = InMemoryNetwork::new();
    let dir = temp_dir("s3gate");
    let ca = new_ca(&dir);
    let nodes = build_company_network(&net, &dir, &["a", "b", "c"], &ca);

    let res = nodes[0].svc.ask(SHAREABLE_QUESTION);
    assert!(res.shareable);
    assert!(res.announced_to >= 2, "2ノード以上(B,C)へ配送");
    for n in &nodes[1..]
    {
        assert!(
            n.svc.cache().lock().unwrap().contains(&res.entry_id),
            "全ピアが共有エントリを保持: {}",
            n.node_id
        );
    }
}

#[test]
fn receiver_rederives_state_over_network()
{
    // A が登録した shareable=true エントリを B は Transfer(core only)から
    // 自前再導出する。B 側の share_reason に「[再導出]」印が付くことで
    // 「送信者の state を運ばず受信側が導出した」ことを観測する。
    let net = InMemoryNetwork::new();
    let dir = temp_dir("rederive_net");
    let ca = new_ca(&dir);
    let nodes = build_company_network(&net, &dir, &["a", "b"], &ca);
    let (a, b) = (&nodes[0], &nodes[1]);

    let res = a.svc.ask(SHAREABLE_QUESTION);
    let detail = b.svc.entry_detail(&res.entry_id).expect("B に取り込まれている");
    assert!(detail.shareable);
    assert!(
        detail.share_reason.starts_with("[再導出]"),
        "受信側再導出の印: {}",
        detail.share_reason
    );
    // 追跡可能性: author_node_id が author_pub から再計算されている。
    assert!(!detail.author_node_id.is_empty());
}

// ------------------------------------------------------------------
// anti-entropy(announce 損失の補償)
// ------------------------------------------------------------------

#[test]
fn anti_entropy_recovers_dropped_announce()
{
    let net = InMemoryNetwork::new();
    let dir = temp_dir("antientropy");
    let ca = new_ca(&dir);
    let nodes = build_company_network(&net, &dir, &["a", "b"], &ca);
    let (a, b) = (&nodes[0], &nodes[1]);

    // announce を疑似ロスさせる(best-effort の「届かない」)。
    net.set_drop_announces(true);
    let res = a.svc.ask(SHAREABLE_QUESTION);
    assert_eq!(a.svc.entry_count(), 1);
    assert_eq!(b.svc.entry_count(), 0, "announce 損失で B は未取得");

    // Digest 交換で欠落分を検出しプルする。
    let rep = b.svc.run_anti_entropy_once();
    assert_eq!(rep.pulled, 1, "欠落分がプルされる");
    assert_eq!(b.svc.entry_count(), 1);
    assert!(b.svc.cache().lock().unwrap().contains(&res.entry_id));

    // 収束後は digest 一致で全件比較を省略(pulled=0・digests_matched=1)。
    let rep2 = b.svc.run_anti_entropy_once();
    assert_eq!(rep2.pulled, 0);
    assert_eq!(rep2.digests_matched, 1, "digest_hash 一致で収束");
}

// ------------------------------------------------------------------
// author_sig / CA検証(CRL失効 → drop)
// ------------------------------------------------------------------

#[test]
fn crl_revoked_author_is_dropped_over_network()
{
    let net = InMemoryNetwork::new();
    let dir = temp_dir("crl_drop");
    let ca = new_ca(&dir);
    let nodes = build_company_network(&net, &dir, &["a", "b"], &ca);
    let (a, b) = (&nodes[0], &nodes[1]);

    // announce をロスさせ、ingest_transfer 単経路を隔離して検証する。
    net.set_drop_announces(true);
    let res = a.svc.ask(SHAREABLE_QUESTION);
    let transfer = a.svc.handle_entry_request(&res.entry_id).expect("A は共有可を供出");
    assert_eq!(b.svc.entry_count(), 0);

    // A の cert(node_id)を B の CRL に載せる → 以後 A 由来は受信拒否。
    b.cert_policy.set_crl(Crl { revoked: vec![a.cert.node_id.clone()] });
    let r = b.svc.ingest_transfer(&transfer, Some(&res.entry_id));
    assert!(r.is_err(), "CRL 失効ノード由来は drop: {r:?}");
    assert_eq!(b.svc.entry_count(), 0, "失効由来は取り込まれない");
}

#[test]
fn reject_all_cert_policy_drops_ingest()
{
    // private 相当の RejectAllCertPolicy を積んだノードは、正しい署名でも
    // cert 段(手順2)で全拒否する(多層防御。§6)。
    let net = InMemoryNetwork::new();
    let dir = temp_dir("rejectall");
    let ca = new_ca(&dir);
    let nodes = build_company_network(&net, &dir, &["a"], &ca);
    let a = &nodes[0];
    let res = a.svc.ask(SHAREABLE_QUESTION);
    let transfer = a.svc.handle_entry_request(&res.entry_id).unwrap();

    let (identb, _sb) = make_identity(&dir, "b", Mode::Company);
    let peer = Arc::new(PeerTable::new());
    let delivery = Delivery { transport: net.transport(), discovery: peer };
    let svc_b = NodeService::new(
        NodeConfig::new(Mode::Company, dir.join("store_reject")),
        identb,
        shared_embedder(),
        shared_agent(),
        Policies::phase1(Arc::new(RejectAllCertPolicy)),
        Some(delivery),
    )
    .unwrap();

    let r = svc_b.ingest_transfer(&transfer, Some(&res.entry_id));
    assert!(r.is_err(), "RejectAll は cert 段で全拒否");
}

// ------------------------------------------------------------------
// 無効 node_cert のネット経路 drop(§10観点6の補完)
//
// verify_author 単体の否定ケース(test_node.rs)に加えて、期限切れ /
// 別CA発行 / mode不許可 の3種が「ネット越し ingest_transfer 経路」でも
// 手順2(組織PKI検証)で drop されることを検証する。形は
// crl_revoked_author_is_dropped_over_network と同型。
// ------------------------------------------------------------------

// 供出側 A(正規1ノード)から共有可 Transfer を1件取り出す。
fn shareable_transfer_from_a(
    net: &crate::transport::InMemoryNetwork,
    dir: &std::path::Path,
    ca: &Arc<dyn Signer>,
) -> (crate::wire::Transfer, String, String)
{
    let nodes = build_company_network(net, dir, &["a"], ca);
    let a = &nodes[0];
    let res = a.svc.ask(SHAREABLE_QUESTION);
    assert!(res.shareable, "前提: 首都エントリは共有可");
    let transfer = a.svc.handle_entry_request(&res.entry_id).expect("A は共有可を供出");
    (transfer, res.entry_id.clone(), a.author_pub.clone())
}

// 受信ノードを「著者の cert として cert_for_author を登録した状態」で組む。
// ca_verifier / ca_pub は受信側が信頼する CA(正規CA)。
fn receiver_with_author_cert(
    net: &crate::transport::InMemoryNetwork,
    dir: &std::path::Path,
    name: &str,
    ca: &Arc<dyn Signer>,
    cert_for_author: NodeCert,
) -> NodeService
{
    let cert_policy = Arc::new(CompanyCertPolicy::new(ca.clone(), ca.public_key_hex()));
    cert_policy.upsert_cert(cert_for_author);
    let (ident, _s) = make_identity(dir, name, Mode::Company);
    let peer = Arc::new(PeerTable::new());
    let delivery = Delivery { transport: net.transport(), discovery: peer };
    NodeService::new(
        NodeConfig::new(Mode::Company, dir.join(format!("store_{name}"))),
        ident,
        shared_embedder(),
        shared_agent(),
        Policies::phase1(cert_policy),
        Some(delivery),
    )
    .unwrap()
}

#[test]
fn expired_cert_author_is_dropped_over_network()
{
    let net = InMemoryNetwork::new();
    let dir = temp_dir("cert_expired_net");
    let ca = new_ca(&dir);
    let (transfer, eid, author_pub) = shareable_transfer_from_a(&net, &dir, &ca);

    // 対照: 有効期限内の cert を持つ受信ノードは取り込める(経路の生存確認)。
    let ok_cert = issue_node_cert(ca.as_ref(), &author_pub, &future_expiry(), &[Mode::Company]);
    let svc_ok = receiver_with_author_cert(&net, &dir, "b_ok", &ca, ok_cert);
    assert!(
        svc_ok.ingest_transfer(&transfer, Some(&eid)).is_ok(),
        "対照: 有効な cert なら同一 Transfer は取り込める"
    );

    // 本題: 著者の cert が期限切れ → 手順2で drop。
    let expired = issue_node_cert(ca.as_ref(), &author_pub, &past_expiry(), &[Mode::Company]);
    let svc_b = receiver_with_author_cert(&net, &dir, "b_exp", &ca, expired);
    let r = svc_b.ingest_transfer(&transfer, Some(&eid));
    assert!(r.is_err(), "期限切れ cert の著者由来は drop: {r:?}");
    assert_eq!(svc_b.entry_count(), 0, "期限切れ cert 由来は取り込まれない");
}

#[test]
fn cert_from_other_ca_is_dropped_over_network()
{
    let net = InMemoryNetwork::new();
    let dir = temp_dir("cert_wrongca_net");
    let dir2 = temp_dir("cert_wrongca_net_ca2");
    let ca = new_ca(&dir); // 受信側が信頼する正規CA
    let other_ca = new_ca(&dir2); // 別組織のCA(別秘密)
    let (transfer, eid, author_pub) = shareable_transfer_from_a(&net, &dir, &ca);

    // 著者の cert が別CA発行 → 正規CAでのCA署名検証に失敗し drop。
    let foreign = issue_node_cert(other_ca.as_ref(), &author_pub, &future_expiry(), &[Mode::Company]);
    let svc_b = receiver_with_author_cert(&net, &dir, "b_ca2", &ca, foreign);
    let r = svc_b.ingest_transfer(&transfer, Some(&eid));
    assert!(r.is_err(), "別CA発行 cert の著者由来は drop: {r:?}");
    assert_eq!(svc_b.entry_count(), 0, "別CA発行 cert 由来は取り込まれない");
}

#[test]
fn cert_without_company_mode_is_dropped_over_network()
{
    let net = InMemoryNetwork::new();
    let dir = temp_dir("cert_mode_net");
    let ca = new_ca(&dir);
    let (transfer, eid, author_pub) = shareable_transfer_from_a(&net, &dir, &ca);

    // 著者の cert が private のみ許可(company 配送に参加できない鍵)→ drop。
    let private_only = issue_node_cert(ca.as_ref(), &author_pub, &future_expiry(), &[Mode::Private]);
    let svc_b = receiver_with_author_cert(&net, &dir, "b_mode", &ca, private_only);
    let r = svc_b.ingest_transfer(&transfer, Some(&eid));
    assert!(r.is_err(), "company 未許可 cert の著者由来は drop: {r:?}");
    assert_eq!(svc_b.entry_count(), 0, "mode 不許可 cert 由来は取り込まれない");
}

// ------------------------------------------------------------------
// 改ざん検知(ネット越し)
// ------------------------------------------------------------------

#[test]
fn tampered_core_over_network_hash_mismatch()
{
    let net = InMemoryNetwork::new();
    let dir = temp_dir("tamper_core");
    let ca = new_ca(&dir);
    let nodes = build_company_network(&net, &dir, &["a", "b"], &ca);
    let (a, b) = (&nodes[0], &nodes[1]);

    net.set_drop_announces(true);
    let res = a.svc.ask(SHAREABLE_QUESTION);
    let transfer = a.svc.handle_entry_request(&res.entry_id).unwrap();

    // core_b64 を1バイト改変 → 期待 entry_id と不一致で drop。
    let mut bytes = B64.decode(&transfer.envelope.core_b64).unwrap();
    bytes[0] ^= 0x01;
    let tampered = Transfer
    {
        envelope: EntryEnvelope
        {
            core_b64: B64.encode(&bytes),
            ..transfer.envelope.clone()
        },
    };
    let r = b.svc.ingest_transfer(&tampered, Some(&res.entry_id));
    assert!(r.is_err(), "core 改変は HashMismatch で drop: {r:?}");
    assert_eq!(b.svc.entry_count(), 0);
}

#[test]
fn tampered_signature_over_network_bad_signature()
{
    let net = InMemoryNetwork::new();
    let dir = temp_dir("tamper_sig");
    let ca = new_ca(&dir);
    let nodes = build_company_network(&net, &dir, &["a", "b"], &ca);
    let (a, b) = (&nodes[0], &nodes[1]);

    net.set_drop_announces(true);
    let res = a.svc.ask(SHAREABLE_QUESTION);
    let transfer = a.svc.handle_entry_request(&res.entry_id).unwrap();

    // author_sig の先頭 hex を書き換える(cert/hash は通し、署名だけ壊す)。
    let mut chars: Vec<char> = transfer.envelope.author_sig.chars().collect();
    chars[0] = if chars[0] == 'a' { 'b' } else { 'a' };
    let tampered = Transfer
    {
        envelope: EntryEnvelope
        {
            author_sig: chars.into_iter().collect(),
            ..transfer.envelope.clone()
        },
    };
    let r = b.svc.ingest_transfer(&tampered, Some(&res.entry_id));
    assert!(r.is_err(), "署名改変は BadSignature で drop: {r:?}");
    assert_eq!(b.svc.entry_count(), 0);
}

// ------------------------------------------------------------------
// モード分離(§6)
// ------------------------------------------------------------------

#[test]
fn private_with_delivery_is_rejected()
{
    // private + 配送層 → 構造的に拒否(送信経路を持たせない)。
    let net = InMemoryNetwork::new();
    let dir = temp_dir("mode_priv_delivery");
    let (ident, _s) = make_identity(&dir, "p", Mode::Private);
    let peer = Arc::new(PeerTable::new());
    let delivery = Delivery { transport: net.transport(), discovery: peer };
    let r = NodeService::new(
        NodeConfig::new(Mode::Private, dir.join("private_store")),
        ident,
        shared_embedder(),
        shared_agent(),
        Policies::phase1(Arc::new(RejectAllCertPolicy)),
        Some(delivery),
    );
    assert!(r.is_err(), "private + delivery は Err(§6)");
}

#[test]
fn identity_config_mode_mismatch_is_rejected()
{
    let dir = temp_dir("mode_mismatch");
    let (ident, _s) = make_identity(&dir, "x", Mode::Company); // identity=company
    let r = NodeService::new(
        NodeConfig::new(Mode::Private, dir.join("store")), // config=private
        ident,
        shared_embedder(),
        shared_agent(),
        Policies::phase1(Arc::new(RejectAllCertPolicy)),
        None,
    );
    assert!(r.is_err(), "identity と config の mode 不一致は Err");
}

#[test]
fn private_handlers_are_inert()
{
    // 配送層なし(private)のハンドラは全て不活性(構造的不在)。
    let dir = temp_dir("private_inert");
    let (ident, _s) = make_identity(&dir, "p", Mode::Private);
    let svc = NodeService::new(
        NodeConfig::new(Mode::Private, dir.join("private_store")),
        ident,
        shared_embedder(),
        shared_agent(),
        Policies::phase1(Arc::new(RejectAllCertPolicy)),
        None,
    )
    .unwrap();

    let ann = Announce
    {
        entry_id: "x".to_string(),
        question_key: "q".to_string(),
        created: "2026-07-18T00:00:00Z".to_string(),
        node_id: "someone".to_string(),
    };
    assert!(matches!(svc.handle_announce(&ann), AnnounceOutcome::NoDelivery));
    assert!(svc.handle_entry_request("x").is_none(), "private は供出しない");
    assert!(svc.handle_digest_request().entries.is_empty(), "private の Digest は空");
    assert_eq!(svc.run_anti_entropy_once().peers_total, 0, "private に同期経路なし");

    // private でも UI 経路(ask=ローカル)は動くが、配送 announce は0。
    let res = svc.ask(SHAREABLE_QUESTION);
    assert_eq!(res.announced_to, 0, "private は announce しない");
    assert_eq!(svc.entry_count(), 1, "ローカル登録は行われる");
}

// ------------------------------------------------------------------
// TTL 検索除外(物理削除しない)
// ------------------------------------------------------------------

#[test]
fn ttl_excludes_from_search_but_keeps_entry()
{
    let net = InMemoryNetwork::new();
    let dir = temp_dir("ttl_excl");
    let ca = new_ca(&dir);
    // slow TTL を極小(1秒)にしたノード。
    let nodes = build_company_network_cfg(&net, &dir, &["a"], &ca, |c|
    {
        c.slow_ttl_secs = 1;
        c.volatile_ttl_secs = 1;
    });
    let a = &nodes[0];

    // 10日前作成の slow エントリを直接取り込む。
    let q = "アクメ社の本社所在地はどこですか";
    let core = make_core(
        q,
        vec![triple("アクメ社", "本社所在地", "東京")],
        &rfc3339_days_ago(10),
        "slow",
        Tier::Low,
    );
    let id = entry_id(&encode_core(&core));
    a.svc
        .cache()
        .lock()
        .unwrap()
        .ingest_envelope(&envelope_from_core(&core, a.signer.as_ref()), Some(&id))
        .unwrap();
    assert_eq!(a.svc.entry_count(), 1);

    // 同一質問で ask → slow TTL(1秒)を超過 → 検索除外 → miss。
    let res = a.svc.ask(q);
    assert!(!res.hit, "TTL 超過エントリは検索から除外される");
    // 物理削除されていない(元 entry_id は残存する = grow-only)。
    assert!(
        a.svc.cache().lock().unwrap().contains(&id),
        "検索除外でも物理削除はしない"
    );
}

#[test]
fn within_ttl_entry_is_searchable()
{
    // 対照: 既定 TTL(slow=30日)なら 10日前作成は検索対象 → ヒット。
    let net = InMemoryNetwork::new();
    let dir = temp_dir("ttl_within");
    let ca = new_ca(&dir);
    let nodes = build_company_network(&net, &dir, &["a"], &ca);
    let a = &nodes[0];

    let q = "アクメ社の本社所在地はどこですか";
    let core = make_core(
        q,
        vec![triple("アクメ社", "本社所在地", "東京")],
        &rfc3339_days_ago(10),
        "slow",
        Tier::Low,
    );
    let id = entry_id(&encode_core(&core));
    a.svc
        .cache()
        .lock()
        .unwrap()
        .ingest_envelope(&envelope_from_core(&core, a.signer.as_ref()), Some(&id))
        .unwrap();

    let res = a.svc.ask(q);
    assert!(res.hit, "TTL 内なら検索でヒットする");
    assert_eq!(res.entry_id, id);
}

// ------------------------------------------------------------------
// ポリシー差替の非破壊性(§8-2: author_sig 検証コアは差し替え不能)
// ------------------------------------------------------------------

#[test]
fn policy_swap_preserves_signature_core()
{
    let net = InMemoryNetwork::new();
    let dir = temp_dir("polswap");
    let ca = new_ca(&dir);
    let nodes = build_company_network(&net, &dir, &["a"], &ca);
    let a = &nodes[0];
    let res = a.svc.ask(SHAREABLE_QUESTION);
    let valid = a.svc.handle_entry_request(&res.entry_id).unwrap();

    // cert 検証を AllowAll に差し替えたノード(Phase2 差替の予行)。
    let (identb, _sb) = make_identity(&dir, "b", Mode::Company);
    let peer = Arc::new(PeerTable::new());
    let delivery = Delivery { transport: net.transport(), discovery: peer };
    let svc_b = NodeService::new(
        NodeConfig::new(Mode::Company, dir.join("store_allow")),
        identb,
        shared_embedder(),
        shared_agent(),
        Policies::phase1(Arc::new(AllowAllCertPolicy)),
        Some(delivery),
    )
    .unwrap();

    // AllowAll では cert 未登録でも cert 段は通り、正しい署名なら取り込める。
    assert!(
        svc_b.ingest_transfer(&valid, Some(&res.entry_id)).is_ok(),
        "cert を緩めれば cert 段は通る"
    );

    // しかし署名を改変した Transfer は、ポリシーを緩めても必ず drop される
    // (author_sig 検証コアはポリシー差し替えの対象外)。
    let mut chars: Vec<char> = valid.envelope.author_sig.chars().collect();
    chars[0] = if chars[0] == 'a' { 'b' } else { 'a' };
    let tampered = Transfer
    {
        envelope: EntryEnvelope
        {
            author_sig: chars.into_iter().collect(),
            ..valid.envelope.clone()
        },
    };
    assert!(
        svc_b.ingest_transfer(&tampered, Some(&res.entry_id)).is_err(),
        "cert ポリシーを緩めても署名検証コアは不変(§8-2)"
    );
}

// ------------------------------------------------------------------
// 非共有エントリは供出・Digest 掲載されない
// ------------------------------------------------------------------

#[test]
fn non_shareable_entry_is_not_served_or_digested()
{
    let net = InMemoryNetwork::new();
    let dir = temp_dir("nonshare");
    let ca = new_ca(&dir);
    let nodes = build_company_network(&net, &dir, &["a"], &ca);
    let a = &nodes[0];

    // フォールバック回答(生成文)= L2 非事実型で共有除外。
    let res = a.svc.ask("量子コンピュータについて教えてください");
    assert!(!res.shareable, "生成文は共有不可");
    assert_eq!(res.announced_to, 0, "非共有は announce しない");
    assert!(
        a.svc.handle_entry_request(&res.entry_id).is_none(),
        "非共有エントリは Transfer 供出しない"
    );
    let digest = a.svc.handle_digest_request();
    assert!(
        digest.entries.iter().all(|i| i.entry_id != res.entry_id),
        "非共有エントリは Digest に載らない"
    );
}
