// トランスポート抽象(S3設計ノート §3 / §11-4)。
//
// §11-4 採用: メッセージ単位の send/recv 抽象(In-memory / HTTP 差し替え)。
// nyllm-wire/v1 の各メッセージに対応する3操作を trait 化する:
//   send_announce … Announce の push(best-effort)
//   fetch_entry   … Request→Transfer の pull
//   fetch_digest  … Digest の取得(anti-entropy)
// これは既存 Embedder/Signer/Agent と同じ「mock/実装を1コールパスで差替」
// 思想の踏襲(§10)。Phase2 のメッセージ追加(FindNode 等)はメソッド追加で
// 対応する(既存メソッドは変えない)。
//
// InMemoryTransport はテスト用に pub 公開する: N 個の NodeService を
// 1プロセス内で InMemoryNetwork に登録し、チャネルの代わりに共有マップで
// 直接相手ノードのハンドラを呼ぶ(同期実行=テスト決定性)。
// HTTP 実配線(HttpTransport)は feature "http" 配下。

use crate::node::PeerInfo;
use crate::sync::NodeService;
use crate::wire::{Announce, Digest, Transfer};
use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone)]
pub struct TransportError(pub String);

impl fmt::Display for TransportError
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result
    {
        write!(f, "{}", self.0)
    }
}

pub trait Transport: Send + Sync
{
    fn name(&self) -> &str;
    // Announce を1ピアへ送る(push・best-effort。失敗は Err だが呼び出し側は
    // 無視してよい: 取りこぼしは anti-entropy が補償する。§3)
    fn send_announce(&self, peer: &PeerInfo, ann: &Announce) -> Result<(), TransportError>;
    // 実体の要求(Request)→ Transfer(core+署名のみ)の pull
    fn fetch_entry(&self, peer: &PeerInfo, entry_id: &str) -> Result<Transfer, TransportError>;
    // Digest の取得(anti-entropy)
    fn fetch_digest(&self, peer: &PeerInfo) -> Result<Digest, TransportError>;
}

// ------------------------------------------------------------------
// InMemoryTransport(テスト用。§10「プロセス内シミュレーション」)
// ------------------------------------------------------------------

// ノード間を共有マップで繋ぐ仮想ネットワーク。
//   let net = InMemoryNetwork::new();
//   let svc_a = Arc::new(NodeService::new(..., Some(Delivery{
//       transport: net.transport(), discovery: peers }))?);
//   net.register("mem://a", svc_a.clone());
// URL(例 "mem://a")は PeerInfo.url と一致させる。
#[derive(Clone, Default)]
pub struct InMemoryNetwork
{
    nodes: Arc<RwLock<HashMap<String, Arc<NodeService>>>>,
    // announce の疑似ネットワーク損失(§10 anti-entropy テスト用):
    // true の間、send_announce は「成功したが届かない」動作になる。
    drop_announces: Arc<AtomicBool>,
}

impl InMemoryNetwork
{
    pub fn new() -> Self
    {
        Self::default()
    }

    // ノードを仮想ネットワークへ参加させる(url は PeerInfo.url と同じ値)。
    pub fn register(&self, url: &str, node: Arc<NodeService>)
    {
        self.nodes.write().unwrap().insert(url.to_string(), node);
    }

    // 離脱・障害のシミュレーション。
    pub fn unregister(&self, url: &str)
    {
        self.nodes.write().unwrap().remove(url);
    }

    // announce 損失モードの切替(anti-entropy の補償を検証するテスト用)。
    pub fn set_drop_announces(&self, drop: bool)
    {
        self.drop_announces.store(drop, Ordering::SeqCst);
    }

    // このネットワークに接続する Transport を得る(各ノードの Delivery に渡す)。
    pub fn transport(&self) -> Arc<dyn Transport>
    {
        Arc::new(InMemoryTransport { net: self.clone() })
    }

    fn get(&self, url: &str) -> Option<Arc<NodeService>>
    {
        // マップのロックは Arc の取得までに限定する(相手ノードのハンドラ実行中に
        // マップロックを保持しない=折り返し呼び出しでの自己デッドロック防止)
        self.nodes.read().unwrap().get(url).cloned()
    }
}

pub struct InMemoryTransport
{
    net: InMemoryNetwork,
}

impl Transport for InMemoryTransport
{
    fn name(&self) -> &str
    {
        "in-memory"
    }

    fn send_announce(&self, peer: &PeerInfo, ann: &Announce) -> Result<(), TransportError>
    {
        if self.net.drop_announces.load(Ordering::SeqCst)
        {
            // 疑似損失: 送信自体は成功扱い(best-effort の「届かない」ケース)
            return Ok(());
        }
        let node = self
            .net
            .get(&peer.url)
            .ok_or_else(|| TransportError(format!("未登録のピアURL: {}", peer.url)))?;
        // 同期実行(テスト決定性)。HTTP 側では非同期プル起動に相当する処理が
        // 受信ノード内で走る(daemon.rs は spawn_blocking で包む)
        node.handle_announce(ann);
        Ok(())
    }

    fn fetch_entry(&self, peer: &PeerInfo, entry_id: &str) -> Result<Transfer, TransportError>
    {
        let node = self
            .net
            .get(&peer.url)
            .ok_or_else(|| TransportError(format!("未登録のピアURL: {}", peer.url)))?;
        node.handle_entry_request(entry_id)
            .ok_or_else(|| TransportError(format!("エントリ未提供(未知または非共有): {entry_id}")))
    }

    fn fetch_digest(&self, peer: &PeerInfo) -> Result<Digest, TransportError>
    {
        let node = self
            .net
            .get(&peer.url)
            .ok_or_else(|| TransportError(format!("未登録のピアURL: {}", peer.url)))?;
        Ok(node.handle_digest_request())
    }
}

// ------------------------------------------------------------------
// HttpTransport(実配線。feature "http")
// ------------------------------------------------------------------

#[cfg(feature = "http")]
mod http_transport
{
    use super::{Transport, TransportError};
    use crate::node::PeerInfo;
    use crate::wire::{Announce, Digest, Transfer, WireEnvelope, WireMessage, WIRE_VERSION};
    use std::time::Duration;

    // ノード間 HTTP クライアント(§2: エントリ配送はノード間直接HTTP)。
    // reqwest::blocking を使うため、tokio ランタイム上では spawn_blocking の
    // 中から呼ぶこと(daemon.rs / main.rs はその規約を守っている)。
    pub struct HttpTransport
    {
        http: reqwest::blocking::Client,
    }

    impl HttpTransport
    {
        pub fn new() -> Result<Self, TransportError>
        {
            let http = reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .map_err(|e| TransportError(format!("HTTPクライアント初期化失敗: {e}")))?;
            Ok(Self { http })
        }

        fn base(url: &str) -> &str
        {
            url.trim_end_matches('/')
        }

        // 応答の WireEnvelope を検証付きで開封する
        fn open(env: WireEnvelope) -> Result<WireMessage, TransportError>
        {
            if env.wire != WIRE_VERSION
            {
                return Err(TransportError(format!("非対応のwireバージョン: {}", env.wire)));
            }
            Ok(env.msg)
        }
    }

    impl Transport for HttpTransport
    {
        fn name(&self) -> &str
        {
            "http(axum+JSON)"
        }

        fn send_announce(&self, peer: &PeerInfo, ann: &Announce) -> Result<(), TransportError>
        {
            let url = format!("{}/wire/announce", Self::base(&peer.url));
            let env = WireEnvelope::new(WireMessage::Announce(ann.clone()));
            let resp = self
                .http
                .post(url)
                .json(&env)
                .send()
                .map_err(|e| TransportError(format!("announce送信失敗: {e}")))?;
            resp.error_for_status()
                .map(|_| ())
                .map_err(|e| TransportError(format!("announce拒否: {e}")))
        }

        fn fetch_entry(&self, peer: &PeerInfo, entry_id: &str) -> Result<Transfer, TransportError>
        {
            let url = format!("{}/wire/entry/{}", Self::base(&peer.url), entry_id);
            let resp = self
                .http
                .get(url)
                .send()
                .map_err(|e| TransportError(format!("entry取得失敗: {e}")))?
                .error_for_status()
                .map_err(|e| TransportError(format!("entry未提供: {e}")))?;
            let env: WireEnvelope = resp
                .json()
                .map_err(|e| TransportError(format!("Transferパース失敗: {e}")))?;
            match Self::open(env)?
            {
                WireMessage::Transfer(t) => Ok(t),
                other => Err(TransportError(format!("Transfer以外の応答: {other:?}"))),
            }
        }

        fn fetch_digest(&self, peer: &PeerInfo) -> Result<Digest, TransportError>
        {
            let url = format!("{}/wire/digest", Self::base(&peer.url));
            let resp = self
                .http
                .get(url)
                .send()
                .map_err(|e| TransportError(format!("digest取得失敗: {e}")))?
                .error_for_status()
                .map_err(|e| TransportError(format!("digest未提供: {e}")))?;
            let env: WireEnvelope = resp
                .json()
                .map_err(|e| TransportError(format!("Digestパース失敗: {e}")))?;
            match Self::open(env)?
            {
                WireMessage::Digest(d) => Ok(d),
                other => Err(TransportError(format!("Digest以外の応答: {other:?}"))),
            }
        }
    }
}

#[cfg(feature = "http")]
pub use http_transport::HttpTransport;
