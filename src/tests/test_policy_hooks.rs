// ポリシー差し替えフックの実効性テスト(S3設計ノート §8-3 / §8-4)。
//
// Phase1 の既定実装(OrgClockTimePolicy=常にpass / NoRevocationPolicy=常にpass)
// は「フックが存在するが素通し」であり、既存テストではフックが実際に効くこと
// (差し替えれば挙動が変わること)が未検証だった。ここでテスト専用の拒否実装に
// 差し替え、フックが焼き込みでなく本物の差し替え点であることを実証する:
//
//   - TimePolicy(§8-3): 拒否実装に差し替えると ingest_transfer が drop する
//     (cert 段・署名検証コアを通過した後の独立した検証段であることも確認)。
//   - RevocationPolicy(§8-4): エントリ単位失効に差し替えると
//     (a) 検索(ask → is_searchable)から除外される(物理削除はしない)
//     (b) anti-entropy(ピア Digest 由来のプル)が失効 entry_id をプルしない
//     解除すれば復活する = フィルタであって削除ではない(grow-only 維持)。
//
// 注: 供出側の Digest 列挙(handle_digest_request)は Phase1 実装では
// RevocationPolicy を参照しない(shareable と著者CRLのみ)。エントリ単位失効の
// 伝播抑止は受信側のプル前フィルタ(run_anti_entropy_once)が担う設計のため、
// 本テストもその実装点(検索・プル前フィルタ)を検証する。

use super::common::{
    build_company_network, make_identity, new_ca, shared_agent, shared_embedder, temp_dir,
    SHAREABLE_QUESTION,
};
use crate::node::{Mode, PeerInfo};
use crate::policy::{
    CertPolicy, NoRevocationPolicy, Policies, RevocationPolicy, TimePolicy,
};
use crate::sync::{Delivery, NodeConfig, NodeService};
use crate::transport::InMemoryNetwork;
use std::collections::HashSet;
use std::sync::{Arc, RwLock};

// ------------------------------------------------------------------
// テスト専用のポリシー実装(Phase2 差し替えの予行)
// ------------------------------------------------------------------

// 全拒否の時刻検証(§8-3 のアンカー検証差し替えを最も厳しい形で模す)。
struct RejectAllTimePolicy;

impl TimePolicy for RejectAllTimePolicy
{
    fn name(&self) -> &str
    {
        "reject-all-time(テスト専用)"
    }

    fn verify_created(&self, created: &str) -> Result<(), String>
    {
        Err(format!("テスト差し替え: created={created} を拒否"))
    }
}

// 失効リスト式のエントリ単位失効(§8-4 の Phase2 revocation を模す)。
// RwLock で失効の追加・解除を後から切り替えられるようにする。
#[derive(Default)]
struct DenyListRevocationPolicy
{
    revoked: RwLock<HashSet<String>>,
}

impl DenyListRevocationPolicy
{
    fn revoke(&self, entry_id: &str)
    {
        self.revoked.write().unwrap().insert(entry_id.to_string());
    }

    fn unrevoke(&self, entry_id: &str)
    {
        self.revoked.write().unwrap().remove(entry_id);
    }
}

impl RevocationPolicy for DenyListRevocationPolicy
{
    fn name(&self) -> &str
    {
        "deny-list-revocation(テスト専用)"
    }

    fn is_revoked(&self, entry_id: &str) -> bool
    {
        self.revoked.read().unwrap().contains(entry_id)
    }
}

// ------------------------------------------------------------------
// TimePolicy 差し替え(§8-3): 拒否実装なら ingest が drop する
// ------------------------------------------------------------------

#[test]
fn rejecting_time_policy_drops_ingest()
{
    let net = InMemoryNetwork::new();
    let dir = temp_dir("time_reject");
    let ca = new_ca(&dir);
    let nodes = build_company_network(&net, &dir, &["a"], &ca);
    let a = &nodes[0];
    let res = a.svc.ask(SHAREABLE_QUESTION);
    let transfer = a.svc.handle_entry_request(&res.entry_id).expect("A は共有可を供出");

    // 受信ノード B: cert は正規(A と同じ cert 表)、時刻検証のみ拒否に差し替え。
    let make_b = |name: &str, time: Arc<dyn TimePolicy>| -> NodeService
    {
        let (ident, _s) = make_identity(&dir, name, Mode::Company);
        let cert: Arc<dyn CertPolicy> = a.cert_policy.clone();
        let policies = Policies
        {
            cert,
            time,
            revocation: Arc::new(NoRevocationPolicy),
        };
        let delivery = Delivery
        {
            transport: net.transport(),
            discovery: Arc::new(crate::policy::PeerTable::new()),
        };
        NodeService::new(
            NodeConfig::new(Mode::Company, dir.join(format!("store_{name}"))),
            ident,
            shared_embedder(),
            shared_agent(),
            policies,
            Some(delivery),
        )
        .unwrap()
    };

    // 対照: Phase1 既定(スキップ)なら取り込める(cert・署名・ハッシュは正規)。
    let svc_pass = make_b("b_timepass", Arc::new(crate::policy::OrgClockTimePolicy));
    assert!(
        svc_pass.ingest_transfer(&transfer, Some(&res.entry_id)).is_ok(),
        "対照: 既定 TimePolicy なら取り込める"
    );

    // 本題: 拒否実装に差し替えると同一 Transfer が時刻検証段で drop される。
    let svc_reject = make_b("b_timereject", Arc::new(RejectAllTimePolicy));
    let r = svc_reject.ingest_transfer(&transfer, Some(&res.entry_id));
    assert!(r.is_err(), "拒否 TimePolicy では drop される: {r:?}");
    assert!(
        r.as_ref().unwrap_err().contains("時刻検証失敗"),
        "drop 理由は時刻検証段(cert 段や署名段ではない): {r:?}"
    );
    assert_eq!(svc_reject.entry_count(), 0);
}

// ------------------------------------------------------------------
// RevocationPolicy 差し替え(§8-4): 検索除外(物理削除なし)と復活
// ------------------------------------------------------------------

#[test]
fn revocation_policy_excludes_entry_from_search_without_deletion()
{
    let net = InMemoryNetwork::new();
    let dir = temp_dir("revoc_search");
    let ca = new_ca(&dir);
    // 1ノードを手組みし、revocation だけ差し替える(cert は Phase1 正規構成)。
    let revocation = Arc::new(DenyListRevocationPolicy::default());
    let (ident, signer) = make_identity(&dir, "n", Mode::Company);
    let cert_policy = Arc::new(crate::policy::CompanyCertPolicy::new(ca.clone(), ca.public_key_hex()));
    cert_policy.upsert_cert(crate::node::issue_node_cert(
        ca.as_ref(),
        signer.public_key_hex(),
        &super::common::future_expiry(),
        &[Mode::Company],
    ));
    let cert: Arc<dyn CertPolicy> = cert_policy;
    let policies = Policies
    {
        cert,
        time: Arc::new(crate::policy::OrgClockTimePolicy),
        revocation: revocation.clone(),
    };
    let delivery = Delivery
    {
        transport: net.transport(),
        discovery: Arc::new(crate::policy::PeerTable::new()),
    };
    let svc = NodeService::new(
        NodeConfig::new(Mode::Company, dir.join("store_n")),
        ident,
        shared_embedder(),
        shared_agent(),
        policies,
        Some(delivery),
    )
    .unwrap();

    // 登録 → 失効前は検索でヒットする。
    let res = svc.ask(SHAREABLE_QUESTION);
    assert!(!res.hit, "初回はミス(新規登録)");
    let eid = res.entry_id.clone();
    assert!(svc.ask(SHAREABLE_QUESTION).hit, "失効前は検索でヒット");

    // エントリ単位失効 → 検索から除外される(is_searchable フックの実効性)。
    revocation.revoke(&eid);
    assert!(
        !svc.ask(SHAREABLE_QUESTION).hit,
        "失効エントリは検索でヒットしない(§8-4 フックが効く)"
    );
    // 物理削除はしない(grow-only 維持)。
    assert!(
        svc.cache().lock().unwrap().contains(&eid),
        "検索除外でも物理削除はしない"
    );

    // 失効解除で復活する = フィルタであって削除ではない。
    revocation.unrevoke(&eid);
    assert!(svc.ask(SHAREABLE_QUESTION).hit, "失効解除で検索ヒットが復活する");
}

// ------------------------------------------------------------------
// RevocationPolicy 差し替え(§8-4): anti-entropy が失効エントリをプルしない
//
// 【範囲の限定: 本テストが実証するのは「受信側がプルしない」ことのみ】
// RevocationPolicy(エントリ単位失効)を参照するのは受信側のプル前フィルタ
// だけであり、ソース側の供出(handle_entry_request)と Digest 列挙は
// RevocationPolicy を参照しない(それらが照合するのは著者単位の CRL のみ)。
// つまり失効エントリはソース側の Digest に載り続け、直接リクエストされれば
// 供出もされる、という非対称が Phase1 の現状仕様である。
// 【Phase2 申し送り】ソース側(供出・Digest 列挙)にも RevocationPolicy
// フィルタを掛けるべきかを Phase2 で再評価すること。
// ------------------------------------------------------------------

#[test]
fn revocation_policy_blocks_anti_entropy_pull()
{
    let net = InMemoryNetwork::new();
    let dir = temp_dir("revoc_pull");
    let ca = new_ca(&dir);
    // 供出側 A(正規)に共有可エントリを1件用意する。
    let nodes = build_company_network(&net, &dir, &["a"], &ca);
    let a = &nodes[0];
    let res = a.svc.ask(SHAREABLE_QUESTION);
    let eid = res.entry_id.clone();

    // 受信側 B: revocation を差し替え、ピア表に A を静的設定する。
    let revocation = Arc::new(DenyListRevocationPolicy::default());
    let (ident_b, _sb) = make_identity(&dir, "b", Mode::Company);
    let cert: Arc<dyn CertPolicy> = a.cert_policy.clone();
    let policies = Policies
    {
        cert,
        time: Arc::new(crate::policy::OrgClockTimePolicy),
        revocation: revocation.clone(),
    };
    let peer_table = Arc::new(crate::policy::PeerTable::new());
    peer_table.set_peers(vec![PeerInfo
    {
        node_id: a.node_id.clone(),
        url: a.url.clone(),
        node_cert: Some(a.cert.clone()),
    }]);
    let delivery = Delivery { transport: net.transport(), discovery: peer_table };
    let svc_b = NodeService::new(
        NodeConfig::new(Mode::Company, dir.join("store_b")),
        ident_b,
        shared_embedder(),
        shared_agent(),
        policies,
        Some(delivery),
    )
    .unwrap();

    // 失効中: A の Digest には載っているが、B はプル前フィルタで取り込まない。
    revocation.revoke(&eid);
    let rep = svc_b.run_anti_entropy_once();
    assert_eq!(rep.peers_total, 1, "同期対象は A");
    assert_eq!(rep.pulled, 0, "失効エントリはプルしない(§8-4 フックが効く)");
    assert!(
        !svc_b.cache().lock().unwrap().contains(&eid),
        "失効エントリは取り込まれない"
    );

    // 失効解除 → 同じ anti-entropy 経路でプルされる(フィルタが原因だった証左)。
    revocation.unrevoke(&eid);
    let rep2 = svc_b.run_anti_entropy_once();
    assert_eq!(rep2.pulled, 1, "失効解除後はプルされる");
    assert!(svc_b.cache().lock().unwrap().contains(&eid));
}
