// S3(Company Phase1)テスト共通ヘルパー(設計ノート §10)。
//
// マルチノードは「プロセス内シミュレーション」で検証する(§10):
//   InMemoryNetwork に N 個の NodeService を登録し、InMemoryTransport
//   (同期実行=決定性)で announce→プル→ingest を1スレッドで通す。
//
// DummySigner(既定ビルド)の制約(fable 申し送り):
//   MAC は「他ノード鍵」を検証できない(verify は自ノード鍵のみ真)。
//   このため複数著者クロス検証は
//     - 既定ビルド: 全ノードに同一 key_path(同一秘密)を共有させ、
//       ルーティング用 node_id のみをテスト側で一意化する。
//     - ed25519 ビルド: 各ノードが独立鍵を持ち、真のクロス著者検証になる。
//   本ヘルパーはこの差を feature ゲートで吸収し、同一テスト本体が
//   両ビルドで通るようにする(§10「ed25519 feature でも全テスト通過」)。

use crate::agent::{Agent, MockAgent};
use crate::embedder::{Embedder, MockEmbedder};
use crate::entry::{encode_core, EntryEnvelope, ImmutableCore, Provenance, Tier};
use crate::node::{issue_node_cert, Mode, NodeCert, NodeIdentity, PeerInfo};
use crate::policy::{CompanyCertPolicy, PeerTable, Policies};
use crate::signer::{create_signer, Signer};
use crate::sync::{Delivery, NodeConfig, NodeService};
use crate::triples::FactTriple;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chrono::{Duration, Utc};
use rand::Rng;
use std::path::{Path, PathBuf};
use std::sync::Arc;

// ------------------------------------------------------------------
// 一時ディレクトリ(poc/src/tests/common.rs と同方針)
// ------------------------------------------------------------------

pub fn temp_dir(tag: &str) -> PathBuf
{
    let pid = std::process::id();
    let nonce: u64 = rand::thread_rng().gen();
    let dir = std::env::temp_dir().join(format!("nyllm_core_test_{}_{}_{:016x}", tag, pid, nonce));
    std::fs::create_dir_all(&dir).expect("テスト用一時ディレクトリの作成に失敗");
    dir
}

// ------------------------------------------------------------------
// 鍵とルーティングID(feature で挙動を変える)
// ------------------------------------------------------------------

// ed25519: ノードごとに独立鍵ファイル(真のクロス著者検証)。
#[cfg(feature = "ed25519")]
fn key_path(dir: &Path, name: &str) -> PathBuf
{
    dir.join(format!("{name}.key"))
}

// 既定(DummySigner=MAC): 全ノードで同一秘密を共有しないとクロス検証できない
// ため、共有鍵ファイルを使う(申し送りの制約)。
#[cfg(not(feature = "ed25519"))]
fn key_path(dir: &Path, _name: &str) -> PathBuf
{
    dir.join("shared.key")
}

pub fn new_signer(dir: &Path, name: &str) -> Arc<dyn Signer>
{
    Arc::from(create_signer(&key_path(dir, name)).expect("signer 初期化に失敗"))
}

// CA は必ずノード鍵と別秘密(別ファイル)にする。
pub fn new_ca(dir: &Path) -> Arc<dyn Signer>
{
    Arc::from(create_signer(&dir.join("ca.key")).expect("CA signer 初期化に失敗"))
}

// ルーティング用 node_id。
//   ed25519: 公開鍵から本来の node_id を導出(pub が一意なので衝突しない)。
//   既定    : 共有鍵で pub が同一になるため、ルーティング識別子だけ名前で一意化する
//             (NodeIdentity のフィールドは pub。テスト用の正当な調整)。
#[cfg(feature = "ed25519")]
fn routing_id(_name: &str, signer: &Arc<dyn Signer>) -> String
{
    crate::node::node_id(signer.public_key_hex())
}

#[cfg(not(feature = "ed25519"))]
fn routing_id(name: &str, _signer: &Arc<dyn Signer>) -> String
{
    format!("route-{name}")
}

pub fn future_expiry() -> String
{
    (Utc::now() + Duration::days(365)).format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

pub fn past_expiry() -> String
{
    (Utc::now() - Duration::days(1)).format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

pub fn rfc3339_days_ago(days: i64) -> String
{
    (Utc::now() - Duration::days(days)).format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

// ------------------------------------------------------------------
// 共有可(shareable=true)になる既知の質問/回答
// ------------------------------------------------------------------
// MockAgent は "首都" を含む質問に「日本の首都は東京です。」を返す。
// この1文は decompose_ja_possessive で (日本, 首都, 東京) に完全分解され、
// 述語「首都」は permanent。時事語なし → judge_entry で shareable=true。

pub const SHAREABLE_QUESTION: &str = "日本の首都はどこですか";

// ------------------------------------------------------------------
// エントリ(core / envelope)の手組み(冪等・複数版・改ざんテスト用)
// ------------------------------------------------------------------

pub fn triple(s: &str, p: &str, o: &str) -> FactTriple
{
    FactTriple { s: s.to_string(), p: p.to_string(), o: o.to_string() }
}

// facts + created + initial_volatility_class を指定して ImmutableCore を組む。
pub fn make_core(
    question_norm: &str,
    facts: Vec<FactTriple>,
    created: &str,
    initial_volatility_class: &str,
    initial_tier: Tier,
) -> ImmutableCore
{
    ImmutableCore
    {
        schema_ver: crate::entry::SCHEMA_VER,
        question_norm: question_norm.to_string(),
        facts,
        provenance: Provenance
        {
            agent: "mock".to_string(),
            model: String::new(),
            embedder_model_id: "mock-ngram-hash".to_string(),
        },
        created: created.to_string(),
        initial_volatility_class: initial_volatility_class.to_string(),
        initial_tier,
    }
}

// core を署名して .entry エンベロープ(Transfer が運ぶ形)にする。
pub fn envelope_from_core(core: &ImmutableCore, signer: &dyn Signer) -> EntryEnvelope
{
    let core_bytes = encode_core(core);
    EntryEnvelope
    {
        schema_ver: core.schema_ver,
        core_b64: B64.encode(&core_bytes),
        author_pub: signer.public_key_hex().to_string(),
        author_sig: signer.sign_bytes(&core_bytes),
    }
}

// NodeIdentity を1件組む(共有鍵ビルドでもルーティングIDは一意化される)。
// signer も一緒に返す(手組みエンベロープの署名に使うため)。
pub fn make_identity(dir: &Path, name: &str, mode: Mode) -> (NodeIdentity, Arc<dyn Signer>)
{
    let signer = new_signer(dir, name);
    let nid = routing_id(name, &signer);
    (NodeIdentity { signer: signer.clone(), node_id: nid, mode }, signer)
}

pub fn shared_embedder() -> Arc<dyn Embedder>
{
    Arc::new(MockEmbedder::default())
}

pub fn shared_agent() -> Arc<dyn Agent>
{
    Arc::new(MockAgent)
}

// ------------------------------------------------------------------
// Company マルチノードの一括構築
// ------------------------------------------------------------------

// 1ノード分の観測ハンドル(テストからの検査用に主要要素を公開)。
// url/author_pub/peer_table は現行テストで未使用だが、将来のテスト・
// デバッグ用の観測点として保持する。
#[allow(dead_code)]
pub struct CompanyNode
{
    pub svc: Arc<NodeService>,
    pub url: String,
    pub node_id: String,
    pub author_pub: String,
    pub cert: NodeCert,
    pub cert_policy: Arc<CompanyCertPolicy>,
    pub peer_table: Arc<PeerTable>,
    pub signer: Arc<dyn Signer>,
}

// names の各ノードを Company で立て、InMemoryNetwork に登録し、
// 全ノードのピア表・cert 表を相互に充填する。cfg_fn で NodeConfig を微調整できる
// (TTL テスト等)。ca は呼び出し側が new_ca で用意し使い回す。
pub fn build_company_network_cfg(
    net: &crate::transport::InMemoryNetwork,
    dir: &Path,
    names: &[&str],
    ca: &Arc<dyn Signer>,
    cfg_fn: impl Fn(&mut NodeConfig),
) -> Vec<CompanyNode>
{
    let ca_pub = ca.public_key_hex().to_string();
    let embedder = shared_embedder();
    let agent = shared_agent();
    let expires = future_expiry();

    // 第1パス: 鍵・cert・PeerInfo を用意する。
    struct Raw
    {
        name: String,
        signer: Arc<dyn Signer>,
        author_pub: String,
        node_id: String,
        cert: NodeCert,
        url: String,
    }
    let mut raws: Vec<Raw> = Vec::new();
    for name in names
    {
        let signer = new_signer(dir, name);
        let author_pub = signer.public_key_hex().to_string();
        let nid = routing_id(name, &signer);
        let cert = issue_node_cert(ca.as_ref(), &author_pub, &expires, &[Mode::Company]);
        raws.push(Raw
        {
            name: (*name).to_string(),
            signer,
            author_pub,
            node_id: nid,
            cert,
            url: format!("mem://{name}"),
        });
    }

    let peers: Vec<PeerInfo> = raws
        .iter()
        .map(|r| PeerInfo
        {
            node_id: r.node_id.clone(),
            url: r.url.clone(),
            node_cert: Some(r.cert.clone()),
        })
        .collect();
    let all_certs: Vec<NodeCert> = raws.iter().map(|r| r.cert.clone()).collect();

    // 第2パス: NodeService を組んでネットワークへ登録する。
    let mut out: Vec<CompanyNode> = Vec::new();
    for r in raws
    {
        let cert_policy = Arc::new(CompanyCertPolicy::new(ca.clone(), &ca_pub));
        for c in &all_certs
        {
            cert_policy.upsert_cert(c.clone());
        }
        let peer_table = Arc::new(PeerTable::new());
        peer_table.set_peers(peers.clone());
        let policies = Policies::phase1(cert_policy.clone());
        let delivery = Delivery
        {
            transport: net.transport(),
            discovery: peer_table.clone(),
        };
        let identity = NodeIdentity
        {
            signer: r.signer.clone(),
            node_id: r.node_id.clone(),
            mode: Mode::Company,
        };
        let mut config = NodeConfig::new(Mode::Company, dir.join(format!("store_{}", r.name)));
        cfg_fn(&mut config);
        let svc = Arc::new(
            NodeService::new(config, identity, embedder.clone(), agent.clone(), policies, Some(delivery))
                .expect("NodeService 構築に失敗"),
        );
        net.register(&r.url, svc.clone());
        out.push(CompanyNode
        {
            svc,
            url: r.url,
            node_id: r.node_id,
            author_pub: r.author_pub,
            cert: r.cert,
            cert_policy,
            peer_table,
            signer: r.signer,
        });
    }
    out
}

// 既定 NodeConfig 版。
pub fn build_company_network(
    net: &crate::transport::InMemoryNetwork,
    dir: &Path,
    names: &[&str],
    ca: &Arc<dyn Signer>,
) -> Vec<CompanyNode>
{
    build_company_network_cfg(net, dir, names, ca, |_| {})
}
