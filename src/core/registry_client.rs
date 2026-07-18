// レジストリクライアント(S3設計ノート §2 / §9 レジストリAPI)。feature "http"。
//
//   POST /registry/join {node_id, url, node_cert} … 参加登録
//   GET  /registry/peers                          … ピア一覧
//   GET  /registry/ca                             … CA公開鍵 + CRL の配布
//
// ★不変条件(§0・§8-1 最重要): レジストリは「発見」のみを担う。ここで取得した
// ピア一覧・node_cert・CA束は全て未検証データとして扱い、信頼判断
// (cert検証・署名検証)は各ノードの CertPolicy / cache::verify_envelope が
// 自律的に行う。「レジストリにあるから信頼」はこのモジュールにも呼び出し側にも
// 焼き込まない。エントリデータはレジストリを一切通らない。
//
// モード分離(§6): このモジュールをインスタンス化するのは company モードの
// main.rs だけである。private では RegistryClient を生成しない。

use crate::node::{CaBundle, NodeCert, PeerInfo};
use crate::policy::{CompanyCertPolicy, PeerTable};
use serde::Serialize;
use std::time::Duration;

pub struct RegistryClient
{
    base: String,
    http: reqwest::blocking::Client,
}

#[derive(Serialize)]
struct JoinBody<'a>
{
    node_id: &'a str,
    url: &'a str,
    node_cert: &'a NodeCert,
}

impl RegistryClient
{
    pub fn new(base_url: &str) -> Result<Self, String>
    {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| format!("HTTPクライアント初期化失敗: {e}"))?;
        Ok(Self { base: base_url.trim_end_matches('/').to_string(), http })
    }

    // 参加登録(§9)。node_cert はレジストリに預けるだけで、レジストリは
    // 検証しない(受信各ノードが検証する)。
    pub fn join(&self, node_id: &str, url: &str, node_cert: &NodeCert) -> Result<(), String>
    {
        let body = JoinBody { node_id, url, node_cert };
        self.http
            .post(format!("{}/registry/join", self.base))
            .json(&body)
            .send()
            .map_err(|e| format!("join送信失敗: {e}"))?
            .error_for_status()
            .map(|_| ())
            .map_err(|e| format!("join拒否: {e}"))
    }

    // ピア一覧の取得。個々のピアJSONが壊れていても他のピアの取得は続行する
    // (レジストリ経由データは未検証前提のため、寛容にパースして
    //  検証は CertPolicy に委ねる)。
    pub fn peers(&self) -> Result<Vec<PeerInfo>, String>
    {
        let v: serde_json::Value = self
            .http
            .get(format!("{}/registry/peers", self.base))
            .send()
            .map_err(|e| format!("peers取得失敗: {e}"))?
            .error_for_status()
            .map_err(|e| format!("peers応答エラー: {e}"))?
            .json()
            .map_err(|e| format!("peersパース失敗: {e}"))?;
        let mut peers: Vec<PeerInfo> = Vec::new();
        if let Some(arr) = v.get("peers").and_then(|p| p.as_array())
        {
            for item in arr
            {
                match serde_json::from_value::<PeerInfo>(item.clone())
                {
                    Ok(p) => peers.push(p),
                    Err(e) => println!("[registry_client] 不正なピア項目をスキップ: {e}"),
                }
            }
        }
        Ok(peers)
    }

    // CA公開鍵 + CRL の取得(配布点としてのレジストリ。信頼判断は各ノード)。
    pub fn ca(&self) -> Result<CaBundle, String>
    {
        self.http
            .get(format!("{}/registry/ca", self.base))
            .send()
            .map_err(|e| format!("ca取得失敗: {e}"))?
            .error_for_status()
            .map_err(|e| format!("ca応答エラー: {e}"))?
            .json()
            .map_err(|e| format!("caパース失敗: {e}"))
    }
}

// レジストリからの定期リフレッシュ1回分(main.rs のポーリングスレッドが呼ぶ):
//   1. ピア一覧 → PeerTable(発見層。§8-4点目)
//   2. ピアの node_cert → CompanyCertPolicy の cert 表(検証は参照時)
//   3. CA束(CA公開鍵 + CRL)→ CompanyCertPolicy
// 返り値は取得できたピア数。
//
// 【M-1: 信頼アンカーの取り扱い(脅威レビュー対応)】
//   - CA公開鍵: ローカル設定(--ca-pub / --dev-ca-key)でピン留め済みなら
//     レジストリ供給値は反映されない。未設定時のみ初回供給で固定される
//     (TOFU ブートストラップ。以後の供給値も無視 = 毎ポーリングの無条件
//     上書きはしない)。判断は CompanyCertPolicy::set_ca_pub 側に集約。
//   - CRL: Phase1 は無署名・平文HTTP配布のため全置換のまま受け入れる。
//     レジストリ compromise 時に CRL 改ざん(失効ノード復活・正規ノード検閲)
//     が成立しうることは Phase1 既知制約(policy.rs set_crl のコメント参照。
//     Phase2 で CA署名付きCRL + CA pub アウトオブバンド固定を実装する)。
pub fn refresh_once(
    reg: &RegistryClient,
    table: &PeerTable,
    certs: &CompanyCertPolicy,
) -> Result<usize, String>
{
    let peers = reg.peers()?;
    certs.install_peer_certs(&peers);
    let n = peers.len();
    table.set_peers(peers);
    match reg.ca()
    {
        Ok(bundle) =>
        {
            if certs.set_ca_pub(&bundle.ca_pub)
            {
                // 未ピン状態からの初回ブートストラップ時のみ通る(以後は固定)
                println!(
                    "[registry_client] CA公開鍵をレジストリ供給でブートストラップ: {}...",
                    &bundle.ca_pub[..16.min(bundle.ca_pub.len())]
                );
            }
            certs.set_crl(bundle.crl);
        }
        Err(e) => println!("[registry_client] CA束の取得失敗(前回値で継続): {e}"),
    }
    Ok(n)
}
