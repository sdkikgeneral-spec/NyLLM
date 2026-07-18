// HTTP 実配線スモークテスト(S3設計ノート §10「HTTP実配線は別途スモーク」)。
// feature "http"(既定 on)でのみコンパイルされる。
//
// 最小の round-trip を1件確認する:
//   - company ノード: /v1/status が 200、mode=company。/wire/digest が 200
//     (wire ルートがマウントされる)。/v1/ask が 200。
//   - private ノード: /v1/status は 200 だが /wire/digest は 404
//     (wire ルート非マウント = 受信面の構造的不在。§6)。
//
// 実処理は sync::NodeService に委譲されるため(daemon.rs は HTTP 境界のみ)、
// ここでは「HTTP でエンドポイントが期待どおり生えている/生えていない」ことの
// 確認に絞る(ロジックの網羅は test_sync.rs のプロセス内テストが担う)。

use super::common::{
    build_company_network, make_identity, new_ca, shared_agent, shared_embedder, temp_dir,
    SHAREABLE_QUESTION,
};
use crate::daemon::{ui_router, wire_router};
use crate::node::Mode;
use crate::policy::{Policies, RejectAllCertPolicy};
use crate::sync::{NodeConfig, NodeService};
use crate::transport::InMemoryNetwork;
use std::net::TcpListener as StdTcpListener;
use std::sync::Arc;
use std::time::Duration;

// svc を ephemeral ポートで待ち受け、その URL を返す(サーバは背景スレッドで常駐)。
fn spawn_server(svc: Arc<NodeService>) -> String
{
    let std_listener = StdTcpListener::bind("127.0.0.1:0").expect("bind に失敗");
    std_listener.set_nonblocking(true).unwrap();
    let addr = std_listener.local_addr().unwrap();
    let is_company = svc.mode() == Mode::Company;

    std::thread::spawn(move ||
    {
        let rt = tokio::runtime::Runtime::new().expect("tokio ランタイム生成に失敗");
        rt.block_on(async move
        {
            let listener = tokio::net::TcpListener::from_std(std_listener).unwrap();
            let mut app = ui_router(svc.clone());
            if is_company
            {
                // daemon::serve と同じ分岐(company のみ wire をマウント)。
                app = app.merge(wire_router(svc.clone()));
            }
            axum::serve(listener, app).await.unwrap();
        });
    });

    let base = format!("http://{addr}");
    // 起動待ち(status が引けるまでポーリング)。
    let client = reqwest::blocking::Client::new();
    for _ in 0..80
    {
        if client
            .get(format!("{base}/v1/status"))
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

#[test]
fn http_company_endpoints_roundtrip()
{
    let net = InMemoryNetwork::new();
    let dir = temp_dir("http_company");
    let ca = new_ca(&dir);
    let nodes = build_company_network(&net, &dir, &["a"], &ca);
    let base = spawn_server(nodes[0].svc.clone());
    let client = reqwest::blocking::Client::new();

    // /v1/status → 200 & mode=company
    let status: serde_json::Value = client
        .get(format!("{base}/v1/status"))
        .send()
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(status["mode"], "company");

    // /v1/ask → 200(共有可エントリを登録できる)
    let ask: serde_json::Value = client
        .post(format!("{base}/v1/ask"))
        .json(&serde_json::json!({ "question": SHAREABLE_QUESTION }))
        .send()
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(ask["shareable"], true);

    // /wire/digest → 200(company は wire ルートがマウントされている)
    let digest = client.get(format!("{base}/wire/digest")).send().unwrap();
    assert!(digest.status().is_success(), "company は /wire/digest が有効");
}

#[test]
fn http_private_does_not_mount_wire()
{
    let dir = temp_dir("http_private");
    let (ident, _s) = make_identity(&dir, "p", Mode::Private);
    let svc = Arc::new(
        NodeService::new(
            NodeConfig::new(Mode::Private, dir.join("private_store")),
            ident,
            shared_embedder(),
            shared_agent(),
            Policies::phase1(Arc::new(RejectAllCertPolicy)),
            None,
        )
        .unwrap(),
    );
    let base = spawn_server(svc);
    let client = reqwest::blocking::Client::new();

    // /v1/status は生きている。
    let status = client.get(format!("{base}/v1/status")).send().unwrap();
    assert!(status.status().is_success());

    // /wire/digest はマウントされていない → 404(受信面の構造的不在。§6)。
    let digest = client.get(format!("{base}/wire/digest")).send().unwrap();
    assert_eq!(digest.status().as_u16(), 404, "private は /wire/* 非マウント");
}
