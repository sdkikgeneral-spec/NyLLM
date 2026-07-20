// ノードデーモン(axum)— UI向けAPIとノード間APIの2系統(S3設計ノート §9)。
// feature "http"。
//
// UI向け(同一ノード内。Blazor が叩く):
//   POST /v1/ask {question}        → AskResult(検索→ミス時推論→judge→登録→announce)
//   GET  /v1/entries/{entry_id}    → EntryDetail(facts/provenance/volatility)
//   GET  /v1/status                → StatusReport(mode/ピア数/エントリ数/embedder/共有状態)
//   POST /v1/sharing {enabled}     → 共有キルスイッチの実行中トグル(共有オフ+法的姿勢
//                                     再定義スペック §3.2。UI消費用API契約。private でも
//                                     マウントするが常に sharing_active=false のまま無害)
//
// ノード間(core←→core、nyllm-wire/v1):
//   POST /wire/announce            → 202(未知なら非同期プル起動)
//   GET  /wire/entry/{entry_id}    → WireEnvelope(Transfer) / 404
//   GET  /wire/digest              → WireEnvelope(Digest)
//
// モード分離(§6): serve() は private モードでは wire ルートをマウントしない
// (受信面でも配送プロトコルが構造的に不在。送信面は NodeService の
//  delivery=None が保証する)。
//
// 実処理は全て sync::NodeService(トランスポート非依存のデーモンロジック)へ
// 委譲し、ここでは HTTP 境界(JSON・ステータスコード・spawn_blocking)のみを扱う。
// NodeService は同期実装(reqwest::blocking を含む)のため、tokio ランタイムを
// ブロックしないよう必ず spawn_blocking で包む。

use crate::agent::AgentError;
use crate::node::Mode;
use crate::sync::NodeService;
use crate::wire::{WireEnvelope, WireMessage, WIRE_VERSION};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

type Svc = Arc<NodeService>;

#[derive(Deserialize)]
pub struct AskRequest
{
    pub question: String,
}

// POST /v1/sharing の入力(共有キルスイッチ§3.2)。
#[derive(Deserialize)]
pub struct SharingRequest
{
    pub enabled: bool,
}

// ------------------------------------------------------------------
// UI向けルータ
// ------------------------------------------------------------------

pub fn ui_router(svc: Svc) -> Router
{
    Router::new()
        .route("/v1/ask", post(ask_handler))
        .route("/v1/entries/{entry_id}", get(entry_detail_handler))
        .route("/v1/status", get(status_handler))
        .route("/v1/sharing", post(sharing_handler))
        .with_state(svc)
}

async fn ask_handler(
    State(svc): State<Svc>,
    Json(req): Json<AskRequest>,
) -> Result<Json<Value>, (StatusCode, String)>
{
    let result = tokio::task::spawn_blocking(move || svc.ask(&req.question))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("ask実行失敗: {e}")))?;
    // 推論先(Agent)の失敗はゲートウェイ系エラーにマップする(設計 2026-07-18 §4:
    // 自ノードの障害ではなく上流=推論先の障害。エントリは登録されていない)。
    //   Timeout → 504 / それ以外(到達不能・非2xx・パース失敗)→ 502
    let result = result.map_err(|e| match e
    {
        AgentError::Timeout => (StatusCode::GATEWAY_TIMEOUT, format!("推論先エラー: {e}")),
        _ => (StatusCode::BAD_GATEWAY, format!("推論先エラー: {e}")),
    })?;
    Ok(Json(json!(result)))
}

async fn entry_detail_handler(
    State(svc): State<Svc>,
    Path(entry_id): Path<String>,
) -> Result<Json<Value>, StatusCode>
{
    let detail = tokio::task::spawn_blocking(move || svc.entry_detail(&entry_id))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    match detail
    {
        Some(d) => Ok(Json(json!(d))),
        None => Err(StatusCode::NOT_FOUND),
    }
}

async fn status_handler(State(svc): State<Svc>) -> Result<Json<Value>, StatusCode>
{
    let status = tokio::task::spawn_blocking(move || svc.status())
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!(status)))
}

// POST /v1/sharing(共有キルスイッチ§3.2): 実行中トグル(再起動不要)。
// private でもマウントされるが、delivery=None のため sharing_active は常に false
// (§6の構造的不在は置き換えない。無害)。
async fn sharing_handler(
    State(svc): State<Svc>,
    Json(req): Json<SharingRequest>,
) -> Result<Json<Value>, StatusCode>
{
    let (sharing_enabled, sharing_active) = tokio::task::spawn_blocking(move ||
    {
        svc.set_sharing_enabled(req.enabled);
        let status = svc.status();
        (status.sharing_enabled, status.sharing_active)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "sharing_enabled": sharing_enabled, "sharing_active": sharing_active })))
}

// ------------------------------------------------------------------
// ノード間(wire)ルータ
// ------------------------------------------------------------------

pub fn wire_router(svc: Svc) -> Router
{
    Router::new()
        .route("/wire/announce", post(wire_announce_handler))
        .route("/wire/entry/{entry_id}", get(wire_entry_handler))
        .route("/wire/digest", get(wire_digest_handler))
        .with_state(svc)
}

// Announce 受信(§3・§9): 202 を即返し、プル(検証・マージ込み)は
// 非同期タスクで起動する(announce は best-effort 通知のため送信側を待たせない)。
async fn wire_announce_handler(
    State(svc): State<Svc>,
    Json(env): Json<WireEnvelope>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, String)>
{
    if env.wire != WIRE_VERSION
    {
        return Err((StatusCode::BAD_REQUEST, format!("非対応のwireバージョン: {}", env.wire)));
    }
    let WireMessage::Announce(ann) = env.msg
    else
    {
        return Err((StatusCode::BAD_REQUEST, "Announce以外のメッセージ".to_string()));
    };
    // 非同期プル起動(結果は待たない。取りこぼしは anti-entropy が補償)
    tokio::task::spawn_blocking(move ||
    {
        let outcome = svc.handle_announce(&ann);
        println!("[daemon] announce処理: {} → {:?}", &ann.entry_id[..16.min(ann.entry_id.len())], outcome);
    });
    Ok((StatusCode::ACCEPTED, Json(json!({ "accepted": true }))))
}

async fn wire_entry_handler(
    State(svc): State<Svc>,
    Path(entry_id): Path<String>,
) -> Result<Json<WireEnvelope>, StatusCode>
{
    let transfer = tokio::task::spawn_blocking(move || svc.handle_entry_request(&entry_id))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    match transfer
    {
        Some(t) => Ok(Json(WireEnvelope::new(WireMessage::Transfer(t)))),
        None => Err(StatusCode::NOT_FOUND), // 未知 or 非共有(理由は区別して返さない)
    }
}

async fn wire_digest_handler(State(svc): State<Svc>) -> Result<Json<WireEnvelope>, StatusCode>
{
    let digest = tokio::task::spawn_blocking(move || svc.handle_digest_request())
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(WireEnvelope::new(WireMessage::Digest(digest))))
}

// ------------------------------------------------------------------
// サーバ起動
// ------------------------------------------------------------------

// UI向けルータ+(companyのみ)wireルータを1つのリスナで供する。
pub async fn serve(svc: Svc, listen: &str) -> Result<(), String>
{
    let mut app = ui_router(svc.clone());
    if svc.mode() == Mode::Company
    {
        app = app.merge(wire_router(svc.clone()));
    }
    // private では /wire/* が 404(ルート不在)になる = 受信面の構造的不在(§6)
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(|e| format!("{listen} のバインドに失敗: {e}"))?;
    println!(
        "[daemon] {} モードで {} を待受(wire API: {})",
        svc.mode().as_str(),
        listen,
        if svc.mode() == Mode::Company { "有効" } else { "無効" }
    );
    axum::serve(listener, app)
        .await
        .map_err(|e| format!("サーバ実行エラー: {e}"))
}
