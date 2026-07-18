// nyllm-registry ライブラリ部 — 発見専用レジストリのルータ構築(S3設計ノート §2 / §9)。
//
// main.rs(バイナリ)からルータ構築・ハンドラ群を抽出したもの。抽出の目的は
// テスト容易性のみ(nyllm-core の統合テストが実HTTPで「本物のレジストリ
// ハンドラ」を起動できるようにする。§10)。責務・不変条件は main.rs 時代から
// 一切変えていない:
//
// ★不変条件(§0・§8-1 最重要): レジストリは「発見(discovery)」だけを担い、
// 「信頼(trust)」は一切担わない。
//   - node_cert は不透明な JSON 値として保存・中継するだけで、パースも検証も
//     しない(検証は各ノードが行う)。
//   - エントリデータ(.entry / Transfer)はこのプロセスを一切通らない。
//   - CA束は与えられた JSON をそのまま配布する配布点であり、その内容を
//     信頼するかは取得側ノードの判断である。
//   - nyllm-core への依存はゼロ(型すら共有しない=「レジストリにある=信頼」を
//     コード構造としても焼き込まない)。dev 方向の依存(core のテストが本クレートを
//     使う)は許されるが、逆方向は追加しないこと。

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

// ピア1件。node_cert は不透明値(検証しない=信頼判断を持たない)。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PeerRecord
{
    node_id: String,
    url: String,
    #[serde(default)]
    node_cert: Value,
}

#[derive(Clone)]
struct AppState
{
    // node_id → PeerRecord(BTreeMap で一覧順序を決定的に)
    peers: Arc<RwLock<BTreeMap<String, PeerRecord>>>,
    // GET /registry/ca で配布する束(--ca-file の中身そのまま)
    ca: Arc<Value>,
}

async fn join_handler(
    State(st): State<AppState>,
    Json(rec): Json<PeerRecord>,
) -> Result<Json<Value>, (StatusCode, String)>
{
    if rec.node_id.is_empty() || rec.url.is_empty()
    {
        return Err((StatusCode::BAD_REQUEST, "node_id / url は必須".to_string()));
    }
    st.peers.write().unwrap().insert(rec.node_id.clone(), rec);
    Ok(Json(json!({ "ok": true })))
}

async fn peers_handler(State(st): State<AppState>) -> Json<Value>
{
    let peers: Vec<PeerRecord> = st.peers.read().unwrap().values().cloned().collect();
    Json(json!({ "peers": peers }))
}

async fn ca_handler(State(st): State<AppState>) -> Json<Value>
{
    Json((*st.ca).clone())
}

// レジストリのルータを構築する(状態はメモリ内のみ。Phase1 の最小実装)。
// ca には GET /registry/ca でそのまま配布する JSON を与える
// (例: {"ca_pub":"<hex>","crl":{"revoked":[]}})。
pub fn build_router(ca: Value) -> Router
{
    let state = AppState
    {
        peers: Arc::new(RwLock::new(BTreeMap::new())),
        ca: Arc::new(ca),
    };
    Router::new()
        .route("/registry/join", post(join_handler))
        .route("/registry/peers", get(peers_handler))
        .route("/registry/ca", get(ca_handler))
        .with_state(state)
}

// CA束が与えられないときの既定形(main.rs / テストで共用)。
pub fn default_ca_bundle() -> Value
{
    json!({ "ca_pub": "", "crl": { "revoked": [] } })
}
