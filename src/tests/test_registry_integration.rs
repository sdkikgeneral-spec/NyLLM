// レジストリを含む実HTTP統合スモーク(S3設計ノート §2 / §9 / §10)。
// feature "http"(既定 on)でのみコンパイルされる。
//
// test_daemon_http.rs はノード単体のルータ直起動のみで registry_client を
// 通らなかった。本ファイルは「本物のレジストリハンドラ」(nyllm-registry の
// build_router。dev-dependency)と company ノード2つを全て実HTTP
// (エフェメラルポート = 0 番指定で CI 安全)で立て、以下を通す:
//
//   1. join → peers 取得 → ca 取得(RegistryClient / refresh_once の実HTTP経路)
//      - CA公開鍵のピン留め(M-1)と TOFU ブートストラップの両動作を含む
//   2. ノード間 announce → pull → ingest(HttpTransport + daemon wire ルート)
//   3. anti-entropy の Digest 取得(実HTTP)と収束確認
//   4. §10観点8補完: private ノードはレジストリに現れない
//      (main.rs の private 配線 = RegistryClient を生成しない、と同じ構成を
//       ミラーし、観測可能な結果「/registry/peers に載らない」を検証する)
//
// ★不変条件の確認テストでもある(§0・§8-1): レジストリから得たピア・cert・
// CA束は未検証データであり、信頼判断は各ノードの CompanyCertPolicy が行う。
// テスト2はレジストリ供給の偽CAがピン留め済みノードの信頼アンカーを
// 差し替えられないことを実HTTP経路で検証する。

use super::common::{
    future_expiry, make_identity, new_ca, shared_agent, shared_embedder, temp_dir,
    SHAREABLE_QUESTION,
};
use crate::daemon::{ui_router, wire_router};
use crate::node::{issue_node_cert, Mode, NodeCert};
use crate::policy::{CertPolicy, CompanyCertPolicy, PeerTable, Policies, RejectAllCertPolicy};
use crate::registry_client::{refresh_once, RegistryClient};
use crate::signer::Signer;
use crate::sync::{Delivery, NodeConfig, NodeService};
use crate::transport::HttpTransport;
use std::net::TcpListener as StdTcpListener;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

// ------------------------------------------------------------------
// 起動ヘルパー(エフェメラルポート。CI 安全)
// ------------------------------------------------------------------

// axum Router をエフェメラルポートで待ち受け、ベースURLを返す
// (サーバは背景スレッドで常駐。probe_path が 200 を返すまで起動を待つ)。
fn spawn_router(app: axum::Router, probe_path: &str) -> String
{
    let std_listener = StdTcpListener::bind("127.0.0.1:0").expect("bind に失敗");
    std_listener.set_nonblocking(true).unwrap();
    let addr = std_listener.local_addr().unwrap();

    std::thread::spawn(move ||
    {
        let rt = tokio::runtime::Runtime::new().expect("tokio ランタイム生成に失敗");
        rt.block_on(async move
        {
            let listener = tokio::net::TcpListener::from_std(std_listener).unwrap();
            axum::serve(listener, app).await.unwrap();
        });
    });

    let base = format!("http://{addr}");
    let client = reqwest::blocking::Client::new();
    for _ in 0..80
    {
        if client
            .get(format!("{base}{probe_path}"))
            .send()
            .map(|r| r.status().is_success())
            .unwrap_or(false)
        {
            return base;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("HTTP サーバが起動しなかった: {base}");
}

// 本物のレジストリ(nyllm-registry のハンドラ)を実HTTPで起動する。
fn spawn_registry(ca_bundle: serde_json::Value) -> String
{
    spawn_router(nyllm_registry::build_router(ca_bundle), "/registry/peers")
}

// NodeService を daemon と同じ分岐(company のみ wire マウント)で起動する。
fn spawn_node_server(svc: Arc<NodeService>) -> String
{
    let mut app = ui_router(svc.clone());
    if svc.mode() == Mode::Company
    {
        app = app.merge(wire_router(svc.clone()));
    }
    spawn_router(app, "/v1/status")
}

// 条件成立までポーリングする(announce→プルは daemon 側で非同期のため)。
fn wait_until(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool
{
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline
    {
        if cond()
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    cond()
}

// ------------------------------------------------------------------
// company ノード1台分(main.rs の company 配線をテスト用にミラーした構成)
// ------------------------------------------------------------------

struct HttpCompanyNode
{
    svc: Arc<NodeService>,
    url: String,      // 実HTTPサーバのベースURL(join に渡す)
    node_id: String,
    author_pub: String,
    cert: NodeCert,
    cert_policy: Arc<CompanyCertPolicy>,
    peer_table: Arc<PeerTable>,
}

// pinned=true なら CA公開鍵を構築時にピン留め(--ca-pub / --dev-ca-key 相当)、
// false なら空構築(レジストリ供給の TOFU ブートストラップ待ち)。
fn build_http_company_node(
    dir: &Path,
    name: &str,
    ca: &Arc<dyn Signer>,
    pinned: bool,
) -> HttpCompanyNode
{
    let (identity, signer) = make_identity(dir, name, Mode::Company);
    let author_pub = signer.public_key_hex().to_string();
    let node_id = identity.node_id.clone();
    let cert = issue_node_cert(ca.as_ref(), &author_pub, &future_expiry(), &[Mode::Company]);

    let pinned_ca_pub = if pinned { ca.public_key_hex().to_string() } else { String::new() };
    let cert_policy = Arc::new(CompanyCertPolicy::new(ca.clone(), &pinned_ca_pub));
    cert_policy.upsert_cert(cert.clone()); // main.rs と同じく自 cert を登録
    let peer_table = Arc::new(PeerTable::new());
    let policies = Policies::phase1(cert_policy.clone());
    let transport = Arc::new(HttpTransport::new().expect("HttpTransport 初期化に失敗"));
    let delivery = Delivery { transport, discovery: peer_table.clone() };

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
    let url = spawn_node_server(svc.clone());
    HttpCompanyNode
    {
        svc,
        url,
        node_id,
        author_pub,
        cert,
        cert_policy,
        peer_table,
    }
}

// ------------------------------------------------------------------
// 必須1: join → peers → ca(ピン留め/TOFU)→ announce→pull→ingest のスモーク
// ------------------------------------------------------------------

#[test]
fn registry_join_peers_ca_and_http_sync_smoke()
{
    let dir = temp_dir("reg_smoke");
    let ca = new_ca(&dir);

    // 本物のレジストリを正規CA束付きで起動する。
    let reg_url = spawn_registry(serde_json::json!(
    {
        "ca_pub": ca.public_key_hex(),
        "crl": { "revoked": [] }
    }));

    // A = ピン留め(--ca-pub 相当)、B = 未ピン(レジストリ供給の TOFU)。
    let a = build_http_company_node(&dir, "a", &ca, true);
    let b = build_http_company_node(&dir, "b", &ca, false);

    // join(実HTTP)。レジストリは node_cert を不透明値として預かるだけ。
    let reg_a = RegistryClient::new(&reg_url).expect("RegistryClient 初期化に失敗");
    let reg_b = RegistryClient::new(&reg_url).expect("RegistryClient 初期化に失敗");
    reg_a.join(&a.node_id, &a.url, &a.cert).expect("A の join に失敗");
    reg_b.join(&b.node_id, &b.url, &b.cert).expect("B の join に失敗");

    // peers 取得(実HTTP): 両ノードが node_cert 付きで列挙される。
    let peers = reg_a.peers().expect("peers 取得に失敗");
    assert_eq!(peers.len(), 2, "join した2ノードが peers に載る");
    for expected in [&a, &b]
    {
        let p = peers
            .iter()
            .find(|p| p.node_id == expected.node_id)
            .unwrap_or_else(|| panic!("peers に {} が見つからない", expected.node_id));
        assert_eq!(p.url, expected.url, "join した URL がそのまま配布される");
        assert_eq!(
            p.node_cert.as_ref(),
            Some(&expected.cert),
            "node_cert が不透明値のまま中継される(レジストリは検証しない)"
        );
    }

    // ca 取得(実HTTP): 配布点としての CA束。
    let bundle = reg_a.ca().expect("ca 取得に失敗");
    assert_eq!(bundle.ca_pub, ca.public_key_hex(), "CA束がそのまま配布される");
    assert!(bundle.crl.revoked.is_empty());

    // TOFU 前: B は CA 未設定なので他ノード(A)の author を検証できない。
    assert!(
        b.cert_policy.verify_author(&a.author_pub).is_err(),
        "TOFU ブートストラップ前の未ピンノードは検証不可"
    );

    // refresh_once(実HTTP): ピア表 + cert 表 + CA束(TOFU/ピン留め判断込み)。
    let na = refresh_once(&reg_a, &a.peer_table, &a.cert_policy).expect("A の refresh に失敗");
    let nb = refresh_once(&reg_b, &b.peer_table, &b.cert_policy).expect("B の refresh に失敗");
    assert_eq!((na, nb), (2, 2), "refresh_once は取得ピア数を返す");

    // TOFU 後: B はレジストリ供給の CA公開鍵でブートストラップされ、
    // ピア一覧経由で得た A の cert を検証できる。
    assert!(
        b.cert_policy.verify_author(&a.author_pub).is_ok(),
        "TOFU ブートストラップ後は A の author を検証できる"
    );
    // ピン留め済みの A も(元々のピンで)B の author を検証できる。
    assert!(
        a.cert_policy.verify_author(&b.author_pub).is_ok(),
        "ピン留めノードも peers 由来の cert を検証できる"
    );

    // ノード間同期(実HTTP): A の ask → announce → B が pull → ingest。
    let res = a.svc.ask(SHAREABLE_QUESTION).expect("MockAgentは失敗しない");
    assert!(!res.hit, "初回はミス");
    assert!(res.shareable, "首都エントリは共有可");
    assert_eq!(res.announced_to, 1, "B へ announce 送達(202)");

    // announce 処理は daemon 側で非同期(202 即応答)のためポーリングで待つ。
    let eid = res.entry_id.clone();
    assert!(
        wait_until(Duration::from_secs(10), || b.svc.cache().lock().unwrap().contains(&eid)),
        "B が announce→pull→ingest で同一 entry_id を取得する"
    );
    assert_eq!(b.svc.entry_count(), 1);

    // anti-entropy(実HTTPの Digest 取得): 収束済みなら digest 一致で省略。
    let rep = b.svc.run_anti_entropy_once();
    assert_eq!(rep.peers_total, 1, "B の同期対象は A のみ(自分は除外)");
    assert_eq!(rep.pulled, 0, "収束済みなので新規プルなし");
    assert_eq!(rep.digests_matched, 1, "digest_hash 一致で全件比較を省略");
}

// ------------------------------------------------------------------
// 必須1補強(M-1): レジストリ供給の偽CAはピン留め済みアンカーを差し替えられない
//
// 注意: 本テストが検証するのは CA ピン留め防御のみ。同じ compromise レジストリ
// による CRL 検閲(refresh_once の set_crl は無署名 CRL の無条件全置換)は依然
// 成立する Phase1 既知制約(policy.rs:128-137 に記録済み。Phase2 の CA署名付き
// CRL で対処)。
// ------------------------------------------------------------------

#[test]
fn registry_supplied_ca_cannot_override_pin_over_http()
{
    let dir = temp_dir("reg_pin");
    let dir_other = temp_dir("reg_pin_other");
    let ca = new_ca(&dir); // 正規CA(ノードの cert 発行者)
    let other_ca = new_ca(&dir_other); // レジストリが配る偽CA

    // レジストリ(compromise 想定)が偽CA公開鍵を配布している。
    let reg_url = spawn_registry(serde_json::json!(
    {
        "ca_pub": other_ca.public_key_hex(),
        "crl": { "revoked": [] }
    }));

    // A = 正規CAにピン留め、C = 未ピン(TOFU で偽CAを掴む対照ノード)。
    let a = build_http_company_node(&dir, "a", &ca, true);
    let c = build_http_company_node(&dir, "c", &ca, false);

    let reg_a = RegistryClient::new(&reg_url).unwrap();
    let reg_c = RegistryClient::new(&reg_url).unwrap();
    reg_a.join(&a.node_id, &a.url, &a.cert).unwrap();
    reg_c.join(&c.node_id, &c.url, &c.cert).unwrap();
    refresh_once(&reg_a, &a.peer_table, &a.cert_policy).unwrap();
    refresh_once(&reg_c, &c.peer_table, &c.cert_policy).unwrap();

    // ピン留めノード A: 偽CA供給を無視し、正規CAで検証が継続する(M-1)。
    assert!(
        a.cert_policy.verify_author(&a.author_pub).is_ok(),
        "ピン留めノードはレジストリ供給の偽CAに影響されない"
    );
    assert!(
        a.cert_policy.verify_author(&c.author_pub).is_ok(),
        "ピア(正規CA発行 cert)の検証も正規CAで継続する"
    );

    // 対照(未ピン C): 偽CAで TOFU 固定されるため、正規CA発行の cert は
    // 検証に通らない = 偽CA供給が refresh_once を実際に流れたことの証左であり、
    // A が守られたのはピン留めの効果であることを裏付ける。
    assert!(
        c.cert_policy.verify_author(&c.author_pub).is_err(),
        "未ピンノードは偽CAを掴む(Phase1 既知制約 = TOFU の限界)"
    );
}

// ------------------------------------------------------------------
// 必須3(§10観点8補完): private ノードはレジストリに現れない
// ------------------------------------------------------------------

#[test]
fn private_node_never_appears_in_registry()
{
    let dir = temp_dir("reg_private");
    let ca = new_ca(&dir);
    let reg_url = spawn_registry(serde_json::json!(
    {
        "ca_pub": ca.public_key_hex(),
        "crl": { "revoked": [] }
    }));

    // company ノード A は通常どおり join する。
    let a = build_http_company_node(&dir, "a", &ca, true);
    let reg_a = RegistryClient::new(&reg_url).unwrap();
    reg_a.join(&a.node_id, &a.url, &a.cert).unwrap();
    refresh_once(&reg_a, &a.peer_table, &a.cert_policy).unwrap();

    // private ノード P: main.rs の private 配線と同じ構成
    // (RejectAllCertPolicy・delivery=None・RegistryClient を生成しない。§6)。
    // 生きたノードとして実HTTPサーバは立てる(join だけが構造的に存在しない)。
    let (ident_p, _sp) = make_identity(&dir, "p", Mode::Private);
    let p_node_id = ident_p.node_id.clone();
    let svc_p = Arc::new(
        NodeService::new(
            NodeConfig::new(Mode::Private, dir.join("private_store")),
            ident_p,
            shared_embedder(),
            shared_agent(),
            Policies::phase1(Arc::new(RejectAllCertPolicy)),
            None,
        )
        .unwrap(),
    );
    let p_url = spawn_node_server(svc_p.clone());

    // P は稼働している(UI 経路は生きている)が…
    let client = reqwest::blocking::Client::new();
    let status: serde_json::Value = client
        .get(format!("{p_url}/v1/status"))
        .send()
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(status["mode"], "private", "P は private ノードとして稼働中");

    // …レジストリの /registry/peers には company の A だけが載り、P は現れない。
    let peers = reg_a.peers().expect("peers 取得に失敗");
    assert_eq!(peers.len(), 1, "レジストリに載るのは join した company ノードのみ");
    assert_eq!(peers[0].node_id, a.node_id);
    assert!(
        peers.iter().all(|p| p.node_id != p_node_id),
        "private ノードはレジストリに現れない(§6 / §10観点8)"
    );

    // P 自身も発見層を持たない(peers=0)= 発見の両方向で不在。
    assert_eq!(status["peers"], 0, "private はピア発見を持たない");
}
