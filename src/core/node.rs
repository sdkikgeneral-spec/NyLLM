// ノード同一性と組織PKI(S3設計ノート §1「ノード・信頼モデル」)。
//
//   - node_id = hex(sha256(NODE_DOMAIN_TAG || node_pub))。Ed25519公開鍵のハッシュ。
//     ドメイン分離タグ nyllm/node/v1(S2.5 の nyllm/entry/v1・nyllm/qkey/v1 とは
//     別タグ。S2.5 §11 ドメインタグ登録簿の思想を踏襲)。
//   - node_cert = CA_sign(node_id || node_pub || 有効期限 || mode許可)。
//     組織内部CA(PKI)が各ノードの公開鍵に署名した証明書。
//   - author_sig の組織内検証は2段構成(§1):
//       (a) core_bytes に対する author_sig を author_pub で検証
//           (cache::verify_envelope。ポリシーで差し替え不能なコア)
//       (b) その author_pub が有効な node_cert を持つ(CA署名OK・未失効)
//           (policy::CertPolicy。Phase2 で witness/評判検証へ差し替え可能)
//   - CRL: 侵害・誤設定ノードの鍵を組織CAが失効させる(ノード単位のPKI失効)。
//     Phase2 の revocation(エントリ単位失効)とは別物(§7)。混同しない。

use crate::entry::{push_lp_str, sha256_hex};
use crate::signer::{create_signer, Signer};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

// ドメイン分離タグ(§1)。node_id 算出と node_cert 署名対象の両方がこのタグ族に
// 属する。両者は cross-use されないよう、cert 側にはサブコンテキスト
// NODE_CERT_CONTEXT を追加で付与する(タグ登録簿への追記は docs 側課題)。
pub const NODE_DOMAIN_TAG: &[u8] = b"nyllm/node/v1\n";
const NODE_CERT_CONTEXT: &[u8] = b"cert\n";

// ------------------------------------------------------------------
// ネットワークモード(S3設計ノート §6 / Architecture §9)
// ------------------------------------------------------------------

// Company / Private の2モードのみ(over-engineering回避。Public は Phase2 で
// 起動オプションのみ予約し中身未実装とする方針のため、enum にも追加しない)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode
{
    Company,
    Private,
}

impl Mode
{
    pub fn parse(s: &str) -> Option<Mode>
    {
        match s
        {
            "company" => Some(Mode::Company),
            "private" => Some(Mode::Private),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str
    {
        match self
        {
            Mode::Company => "company",
            Mode::Private => "private",
        }
    }
}

// ------------------------------------------------------------------
// NodeId
// ------------------------------------------------------------------

// node_id = hex(sha256(NODE_DOMAIN_TAG || node_pub_bytes))(§1)。
// node_pub_hex は Signer::public_key_hex() の値(hex文字列)。hex として不正な
// 入力は識別子用途のみの防御的縮退として UTF-8 バイト列をそのまま用いる
// (正規の Signer 実装は常に有効な hex を返すため、この分岐は通常通らない)。
pub fn node_id(node_pub_hex: &str) -> String
{
    let mut buf: Vec<u8> = NODE_DOMAIN_TAG.to_vec();
    match hex::decode(node_pub_hex)
    {
        Ok(b) => buf.extend_from_slice(&b),
        Err(_) => buf.extend_from_slice(node_pub_hex.as_bytes()),
    }
    sha256_hex(&buf)
}

// UTC RFC3339・秒精度・Z終端(S2.5 §1 #7 と同形式)。
pub fn now_utc_rfc3339() -> String
{
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

// ------------------------------------------------------------------
// node_cert(組織CA発行の証明書)と CRL
// ------------------------------------------------------------------

// node_cert = CA_sign(node_id || node_pub || 有効期限 || mode許可)(§1)。
// serde はレジストリ経由の配布・ディスク保存用(署名対象は cert_signing_bytes が
// 生成する正準バイト列であり、JSON表現には依存しない = S2.5 §0 と同じ原則)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeCert
{
    pub node_id: String,            // hex(sha256(tag || node_pub))
    pub node_pub: String,           // hex公開鍵
    pub expires: String,            // 有効期限(UTC RFC3339・Z終端)
    pub allowed_modes: Vec<String>, // mode許可("company" / "private")
    pub ca_sig: String,             // CA署名(hex)
}

// node_cert の署名対象バイト列(正準形。lp_str は S2.5 §3 と同一の
// 長さ接頭辞 + NFC 正規化)。
//   NODE_DOMAIN_TAG || NODE_CERT_CONTEXT || lp(node_id) || lp(node_pub)
//   || lp(expires) || u32(モード数) + 各 lp(mode)
pub fn cert_signing_bytes(
    node_id: &str,
    node_pub: &str,
    expires: &str,
    allowed_modes: &[String],
) -> Vec<u8>
{
    let mut buf: Vec<u8> = NODE_DOMAIN_TAG.to_vec();
    buf.extend_from_slice(NODE_CERT_CONTEXT);
    push_lp_str(&mut buf, node_id);
    push_lp_str(&mut buf, node_pub);
    push_lp_str(&mut buf, expires);
    buf.extend_from_slice(&(allowed_modes.len() as u32).to_be_bytes());
    for m in allowed_modes
    {
        push_lp_str(&mut buf, m);
    }
    buf
}

// CA が node_cert を発行する(§11-2: 既存社内PKI流用が推奨だが、PoC検証・テスト用に
// 軽量CA発行関数をコアに置く。ca は CA の Signer 実体)。
pub fn issue_node_cert(
    ca: &dyn Signer,
    node_pub_hex: &str,
    expires: &str,
    allowed_modes: &[Mode],
) -> NodeCert
{
    let id = node_id(node_pub_hex);
    let modes: Vec<String> = allowed_modes.iter().map(|m| m.as_str().to_string()).collect();
    let bytes = cert_signing_bytes(&id, node_pub_hex, expires, &modes);
    NodeCert
    {
        node_id: id,
        node_pub: node_pub_hex.to_string(),
        expires: expires.to_string(),
        allowed_modes: modes,
        ca_sig: ca.sign_bytes(&bytes),
    }
}

// node_cert の検証(§1 (b) の実体。CertPolicy=policy.rs から呼ばれる):
//   1. node_id が node_pub から再計算した値と一致(ID詐称防止)
//   2. CA署名が有効(ca_verifier は検証に使う Signer 実体。Ed25519 では任意の
//      インスタンスで公開検証できる。DummySigner は同一秘密のインスタンスが必要 =
//      MACプレースホルダの既知の限界)
//   3. 有効期限内(now と比較)
// CRL照合と mode許可の確認は呼び出し側(CertPolicy)が行う(§7: 失効=CRL は
// 証明書自体の正しさとは別の判断であるため分離)。
pub fn verify_node_cert(
    ca_verifier: &dyn Signer,
    ca_pub_hex: &str,
    cert: &NodeCert,
    now: DateTime<Utc>,
) -> Result<(), String>
{
    if node_id(&cert.node_pub) != cert.node_id
    {
        return Err("node_id が node_pub と不一致(ID詐称の疑い)".to_string());
    }
    let bytes = cert_signing_bytes(&cert.node_id, &cert.node_pub, &cert.expires, &cert.allowed_modes);
    if !ca_verifier.verify(ca_pub_hex, &cert.ca_sig, &bytes)
    {
        return Err("CA署名の検証に失敗".to_string());
    }
    let expires = DateTime::parse_from_rfc3339(&cert.expires)
        .map_err(|e| format!("有効期限のパースに失敗: {e}"))?;
    if expires.with_timezone(&Utc) <= now
    {
        return Err(format!("node_cert 有効期限切れ({})", cert.expires));
    }
    Ok(())
}

// mode許可の確認(node_cert = 「mode許可」を含む。§1)。
pub fn cert_allows_mode(cert: &NodeCert, mode: Mode) -> bool
{
    cert.allowed_modes.iter().any(|m| m == mode.as_str())
}

// CA失効リスト(CRL。§1「消さないもの」/§7)。ノード単位のPKI失効であり、
// Phase2 のエントリ単位 revocation とは別物。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Crl
{
    pub revoked: Vec<String>, // 失効した node_id の一覧
}

impl Crl
{
    pub fn is_revoked(&self, node_id: &str) -> bool
    {
        self.revoked.iter().any(|r| r == node_id)
    }
}

// レジストリの GET /registry/ca が配布する束(CA公開鍵 + CRL。§9)。
// レジストリは配布点にすぎず、これを信頼するか(検証に使うか)は各ノードの判断
// (§0: レジストリは発見のみ・信頼判断を持たない)。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CaBundle
{
    pub ca_pub: String,
    #[serde(default)]
    pub crl: Crl,
}

// ------------------------------------------------------------------
// ピア情報と自ノードの同一性
// ------------------------------------------------------------------

// ピア1件(レジストリ経由 or テストで静的に構成)。node_cert はレジストリが
// 中継しただけの未検証データであり、信頼判断は受信側 CertPolicy が行う。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeerInfo
{
    pub node_id: String,
    pub url: String, // ノード間APIのベースURL(InMemory では任意の一意キー)
    #[serde(default)]
    pub node_cert: Option<NodeCert>,
}

// 自ノードの同一性(鍵 + node_id + mode)。
pub struct NodeIdentity
{
    pub signer: Arc<dyn Signer>,
    pub node_id: String,
    pub mode: Mode,
}

// 鍵ロード(既存 create_signer を利用: feature に応じて DummySigner /
// Ed25519Signer。鍵ファイルが無ければ生成される)。
pub fn load_identity(key_path: &Path, mode: Mode) -> std::io::Result<NodeIdentity>
{
    let signer: Arc<dyn Signer> = Arc::from(create_signer(key_path)?);
    let node_id = node_id(signer.public_key_hex());
    Ok(NodeIdentity { signer, node_id, mode })
}
