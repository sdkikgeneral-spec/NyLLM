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
    build_company_network, envelope_from_core, future_expiry, make_core, make_identity, new_ca,
    new_signer, rfc3339_days_ago, shared_agent, shared_embedder, temp_dir, triple,
    SHAREABLE_QUESTION,
};
use crate::entry::{encode_core, entry_id, Tier};
use crate::node::{issue_node_cert, node_id, Crl, Mode};
use crate::policy::{
    CertPolicy, CompanyCertPolicy, Layer1TrustPolicy, OrgClockTimePolicy, PeerTable, Policies,
    RevocationPolicy,
};
use crate::signer::Signer;
use crate::sync::{Delivery, NodeConfig, NodeService};
use crate::transport::InMemoryNetwork;
use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, RwLock};

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
    let res = a.svc.ask(SHAREABLE_QUESTION).expect("MockAgentは失敗しない");
    assert!(!res.hit, "初回はミス(新規登録)");
    assert!(res.shareable, "首都エントリは共有可");
    let eid = res.entry_id.clone();

    // 失効前のベースライン: 検索ヒット・供出可・Digest掲載。
    assert!(a.svc.ask(SHAREABLE_QUESTION).expect("MockAgentは失敗しない").hit, "(a前提) 失効前は検索でヒット");
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
        !a.svc.ask(SHAREABLE_QUESTION).expect("MockAgentは失敗しない").hit,
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
    assert!(a.svc.ask(SHAREABLE_QUESTION).expect("MockAgentは失敗しない").hit, "(e) 失効解除で検索ヒットも復活する");
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

// ====================================================================
// S5 §3(d): ソース側 revocation フィルタ対称化(供出・Digest 列挙)
//
// S3 が残した非対称(H-1=著者CRLはソース側にも配線済みだが、
// RevocationPolicy=エントリ単位失効は受信側の2経路(検索除外・
// anti-entropyプル前)にしか配線されていなかった)を、H-1 と同型の
// 遡及除外パターンでソース側2経路(handle_entry_request /
// handle_digest_request)へ拡張したことの回帰テスト。
//
// 既定 NoRevocationPolicy は常に false を返す(Phase1 完全 no-op)ため、
// 「効くこと」を検証するには専用の StubRevocationPolicy を Policies へ
// 注入する必要がある。common.rs::build_company_network は Policies::phase1
// で NoRevocationPolicy をハードコードしており注入できないため(かつ
// common.rs は変更しない方針のため)、test_policy_hooks.rs の
// DenyListRevocationPolicy と同型に、NodeService をこのテストモジュール内で
// 直接組み立てて revocation だけ差し替える。
// ====================================================================

// 指定した entry_id 集合のみ失効(tombstone)扱いにするテスト専用
// RevocationPolicy。RwLock で後から失効集合へ追加できる
// (test_policy_hooks.rs の DenyListRevocationPolicy と同型)。
#[derive(Default)]
struct StubRevocationPolicy
{
    revoked: RwLock<HashSet<String>>,
}

impl StubRevocationPolicy
{
    fn revoke(&self, entry_id: &str)
    {
        self.revoked.write().unwrap().insert(entry_id.to_string());
    }
}

impl RevocationPolicy for StubRevocationPolicy
{
    fn name(&self) -> &str
    {
        "stub-revocation(テスト専用・S5 §3(d) ソース側フィルタ検証用)"
    }

    fn is_revoked(&self, entry_id: &str) -> bool
    {
        self.revoked.read().unwrap().contains(entry_id)
    }
}

// 1ノードを手組みし、revocation だけ StubRevocationPolicy に差し替える。
// cert は Phase1 正規(CompanyCertPolicy に自ノード cert を upsert)、
// time=OrgClockTimePolicy、trust=Layer1TrustPolicy::new()(いずれも
// test_policy_hooks.rs の構成に倣う)。
fn build_node_with_stub_revocation(
    net: &InMemoryNetwork,
    dir: &Path,
    name: &str,
    ca: &Arc<dyn Signer>,
    revocation: Arc<StubRevocationPolicy>,
) -> (Arc<NodeService>, Arc<dyn Signer>)
{
    let (identity, signer) = make_identity(dir, name, Mode::Company);
    let cert_policy = Arc::new(CompanyCertPolicy::new(ca.clone(), ca.public_key_hex()));
    cert_policy.upsert_cert(issue_node_cert(
        ca.as_ref(),
        signer.public_key_hex(),
        &future_expiry(),
        &[Mode::Company],
    ));
    let cert: Arc<dyn CertPolicy> = cert_policy;
    let policies = Policies
    {
        cert,
        time: Arc::new(OrgClockTimePolicy),
        revocation,
        trust: Arc::new(Layer1TrustPolicy::new()),
    };
    let delivery = Delivery
    {
        transport: net.transport(),
        discovery: Arc::new(PeerTable::new()),
    };
    let svc = Arc::new(
        NodeService::new(
            NodeConfig::new(Mode::Company, dir.join(format!("store_{name}"))),
            identity,
            shared_embedder(),
            shared_agent(),
            policies,
            Some(delivery),
        )
        .expect("NodeService 構築に失敗"),
    );
    (svc, signer)
}

// --------------------------------------------------------------
// 1. 供出(handle_entry_request)がエントリ単位失効で None を返す
// --------------------------------------------------------------

#[test]
fn source_side_revoke_blocks_entry_request()
{
    let net = InMemoryNetwork::new();
    let dir = temp_dir("s5_src_entry_request");
    let ca = new_ca(&dir);
    let revocation = Arc::new(StubRevocationPolicy::default());
    let (svc, _signer) = build_node_with_stub_revocation(&net, &dir, "a", &ca, revocation.clone());

    // shareable かつ著者未失効のエントリを1件登録。
    let res = svc.ask(SHAREABLE_QUESTION).expect("MockAgentは失敗しない");
    assert!(res.shareable, "首都エントリは共有可");
    let eid = res.entry_id.clone();

    // 失効前は供出できる(対照)。
    assert!(
        svc.handle_entry_request(&eid).is_some(),
        "失効前提: 失効前は Transfer を供出できる"
    );

    // 当該 entry_id をエントリ単位失効(tombstone)にする。
    revocation.revoke(&eid);

    // 【本題】ソース側フィルタが効き、供出しなくなる(H-1 と同型の遡及除外)。
    assert!(
        svc.handle_entry_request(&eid).is_none(),
        "S5 §3(d): エントリ単位失効は供出(handle_entry_request)も遡及除外する"
    );
}

// --------------------------------------------------------------
// 2. Digest 列挙からエントリ単位失効を除外し、digest_hash も除外後の集合と一致
// --------------------------------------------------------------

#[test]
fn source_side_revoke_excludes_from_digest()
{
    let net = InMemoryNetwork::new();
    let dir = temp_dir("s5_src_digest");
    let ca = new_ca(&dir);
    let revocation = Arc::new(StubRevocationPolicy::default());
    let (svc, signer) = build_node_with_stub_revocation(&net, &dir, "a", &ca, revocation.clone());

    // 失効対象: SHAREABLE_QUESTION エントリ。
    let res = svc.ask(SHAREABLE_QUESTION).expect("MockAgentは失敗しない");
    assert!(res.shareable, "首都エントリは共有可");
    let eid = res.entry_id.clone();

    // 生存対象: 別 question_key の shareable エントリ(直接取り込み。
    // 「本社所在地」はオントロジー既知述語=slow で share_gate を通過する)。
    let survivor_core = make_core(
        "アクメ社の本社所在地はどこですか",
        vec![triple("アクメ社", "本社所在地", "東京")],
        &rfc3339_days_ago(1),
        "slow",
        Tier::Low,
    );
    let eid2 = entry_id(&encode_core(&survivor_core));
    let envelope2 = envelope_from_core(&survivor_core, signer.as_ref());
    svc.cache().lock().unwrap().ingest_envelope(&envelope2, Some(&eid2)).unwrap();

    // 失効前提: 両方とも供出・列挙される。
    assert!(svc.handle_entry_request(&eid).is_some());
    assert!(svc.handle_entry_request(&eid2).is_some());
    let before = svc.handle_digest_request();
    assert!(before.entries.iter().any(|i| i.entry_id == eid));
    assert!(before.entries.iter().any(|i| i.entry_id == eid2));

    // 失効(tombstone)。
    revocation.revoke(&eid);

    let digest = svc.handle_digest_request();
    assert!(
        digest.entries.iter().all(|i| i.entry_id != eid),
        "S5 §3(d): tombstone 済み entry_id は Digest 列挙から除外される"
    );
    assert!(
        digest.entries.iter().any(|i| i.entry_id == eid2),
        "非失効の別エントリは引き続き Digest に列挙される"
    );

    // digest_hash が除外後の集合(生存 id のみ)と一致すること
    // (実装が列挙だけ除外して hash 計算元を揃え忘れる、といった不整合がないことの固定)。
    let expected_hash = crate::wire::digest_hash(&[eid2.clone()]);
    assert_eq!(
        digest.digest_hash, expected_hash,
        "digest_hash は除外後の集合(生存 id のみ)から算出される"
    );
}

// --------------------------------------------------------------
// 3. no-op 回帰: 既定 NoRevocationPolicy では従来どおり供出・列挙される
// --------------------------------------------------------------

#[test]
fn no_revocation_policy_serves_and_lists()
{
    let net = InMemoryNetwork::new();
    let dir = temp_dir("s5_noop");
    let ca = new_ca(&dir);
    // 既定構成(build_company_network = Policies::phase1 = NoRevocationPolicy)。
    let nodes = build_company_network(&net, &dir, &["a"], &ca);
    let a = &nodes[0];

    let res = a.svc.ask(SHAREABLE_QUESTION).expect("MockAgentは失敗しない");
    assert!(res.shareable, "首都エントリは共有可");
    let eid = res.entry_id.clone();

    // 既定 NoRevocationPolicy(常に false)構成では、S5 §3(d) 追加後も
    // 供出・Digest 列挙とも従来どおり(non-op 回帰の固定)。
    assert!(
        a.svc.handle_entry_request(&eid).is_some(),
        "既定 NoRevocationPolicy: 供出は従来どおり行われる(no-op)"
    );
    assert!(
        a.svc.handle_digest_request().entries.iter().any(|i| i.entry_id == eid),
        "既定 NoRevocationPolicy: Digest 列挙も従来どおり(no-op)"
    );
}

// --------------------------------------------------------------
// 4. grow-only 保持: tombstone 済みでも物理削除しない(供出・列挙されないことと両立)
// --------------------------------------------------------------

#[test]
fn revoke_does_not_physically_delete()
{
    let net = InMemoryNetwork::new();
    let dir = temp_dir("s5_growonly");
    let ca = new_ca(&dir);
    let revocation = Arc::new(StubRevocationPolicy::default());
    let (svc, _signer) = build_node_with_stub_revocation(&net, &dir, "a", &ca, revocation.clone());

    let res = svc.ask(SHAREABLE_QUESTION).expect("MockAgentは失敗しない");
    let eid = res.entry_id.clone();

    revocation.revoke(&eid);

    // 供出・列挙はされなくなる。
    assert!(svc.handle_entry_request(&eid).is_none());
    assert!(svc.handle_digest_request().entries.iter().all(|i| i.entry_id != eid));

    // …が、物理保持はされ続ける(grow-only。tombstone はフィルタであって削除ではない)。
    assert!(
        svc.cache().lock().unwrap().contains(&eid),
        "tombstone 済みでも cache.contains は true のまま(物理削除しない)"
    );
    assert!(
        svc.cache().lock().unwrap().get(&eid).is_some(),
        "tombstone 済みでも cache.get は Some のまま(物理削除しない)"
    );
}

// --------------------------------------------------------------
// 5. 同一 question_key の非失効版は巻き添えにならない(entry_id 単位の効き方)
// --------------------------------------------------------------

#[test]
fn non_revoked_sibling_still_served()
{
    let net = InMemoryNetwork::new();
    let dir = temp_dir("s5_sibling");
    let ca = new_ca(&dir);
    let revocation = Arc::new(StubRevocationPolicy::default());
    let (svc, signer) = build_node_with_stub_revocation(&net, &dir, "a", &ca, revocation.clone());

    // v1: SHAREABLE_QUESTION を ask() で登録。
    let res = svc.ask(SHAREABLE_QUESTION).expect("MockAgentは失敗しない");
    assert!(res.shareable);
    let eid1 = res.entry_id.clone();

    // v2: 同一 question_norm(= 同一 question_key)・異なる facts の別版を
    // 直接取り込む(版束ねの検証が目的であり、事実の正誤は関知しない)。
    let core_v2 = make_core(
        SHAREABLE_QUESTION,
        vec![triple("日本", "首都", "大阪")],
        &rfc3339_days_ago(1),
        "permanent",
        Tier::Low,
    );
    let eid2 = entry_id(&encode_core(&core_v2));
    assert_ne!(eid1, eid2, "facts が異なるので entry_id は別になる");
    let envelope_v2 = envelope_from_core(&core_v2, signer.as_ref());
    svc.cache().lock().unwrap().ingest_envelope(&envelope_v2, Some(&eid2)).unwrap();

    // 同一 question_key であることを確認(版束ねの前提)。
    {
        let cache = svc.cache().lock().unwrap();
        let e1 = cache.get(&eid1).expect("v1は存在する");
        let e2 = cache.get(&eid2).expect("v2は存在する");
        assert_eq!(e1.question_key, e2.question_key, "同一 question_key の別版であること");
    }

    // v1 のみを失効(tombstone)。
    revocation.revoke(&eid1);

    // v1 は供出・列挙から外れる。
    assert!(svc.handle_entry_request(&eid1).is_none());
    let digest = svc.handle_digest_request();
    assert!(digest.entries.iter().all(|i| i.entry_id != eid1));

    // v2(非失効の別版)は entry_id 単位で引き続き供出・列挙される(巻き添えなし)。
    assert!(
        svc.handle_entry_request(&eid2).is_some(),
        "同一 question_key の非失効版は供出され続ける(巻き添えなし)"
    );
    assert!(
        digest.entries.iter().any(|i| i.entry_id == eid2),
        "同一 question_key の非失効版は Digest に列挙され続ける(巻き添えなし)"
    );
}
