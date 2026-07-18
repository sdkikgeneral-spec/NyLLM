// ワイヤプロトコル nyllm-wire/v1(S3設計ノート §3「メッセージ型」)。
//
//   Announce  push(通知)  新規登録の通知。実体は含めない
//   Request   pull         実体の要求(HTTPでは GET /wire/entry/{id} のパスに
//                          相当するため本文では使われないが、プロトコル定義として
//                          型を持つ。InMemory等の非HTTPトランスポートが使ってよい)
//   Transfer  pull応答     S2.5 .entry エンベロープ(core+署名のみ)。
//                          mutable_state は送らない(受信側が捨てて再導出する
//                          原則の徹底。§3 要点)
//   Digest    anti-entropy 定期同期の要約。取りこぼし補償
//
// バージョニング(§8「配送抽象のバージョニング」): 全メッセージは
// WireEnvelope{wire: "nyllm-wire/v1", msg} で運ぶ。Phase2 は WireMessage に
// FindNode / WitnessExchange / Revoke 等のバリアントを「追加」する
// (既存バリアントは変えない)。未知バリアントはデシリアライズ失敗として
// 受信側が無視する(前方互換の最小形)。

use crate::entry::{sha256_hex, EntryEnvelope};
use serde::{Deserialize, Serialize};

pub const WIRE_VERSION: &str = "nyllm-wire/v1";

// 新規登録の通知(push・best-effort。§3: 実体を押し付けず、受信側がプルで
// 主導権を持つ)。取りこぼしは Digest 交換で補償する。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Announce
{
    pub entry_id: String,
    pub question_key: String,
    pub created: String,
    pub node_id: String, // 通知元(プル先の解決は受信側がピア表で行う)
}

// 実体の要求(pull)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request
{
    pub entry_id: String,
}

// 実体の転送(pull応答)。core+署名のみ(EntryEnvelope)。
// mutable_state(shareable/tier_operative/volatility_class_operative 等)は
// 構造上ここに存在しない = 「送信者判断を信頼しない」原則が型で保証される。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transfer
{
    pub envelope: EntryEnvelope,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DigestItem
{
    pub entry_id: String,
    pub question_key: String,
}

// anti-entropy 用の要約(§11-5 採用: 定期ポーリング+Digestハッシュ比較)。
// digest_hash が一致すれば全件比較を省略できる。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Digest
{
    pub digest_hash: String,
    pub entries: Vec<DigestItem>,
}

// Digest の比較用ハッシュ。entry_id(固定長hex)昇順の改行連結を sha256 する。
// 署名対象ではない(同一性比較の省力化のみ。なりすまし防止は各エントリの
// 検証パイプラインが担う)ため、ドメインタグは不要。
pub fn digest_hash(sorted_entry_ids: &[String]) -> String
{
    let mut buf: Vec<u8> = Vec::new();
    for id in sorted_entry_ids
    {
        buf.extend_from_slice(id.as_bytes());
        buf.push(b'\n');
    }
    sha256_hex(&buf)
}

// メッセージ本体。serde の内部タグ("type")で判別する。
// Phase2 でのバリアント追加は既存の JSON 表現を変えない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WireMessage
{
    Announce(Announce),
    Request(Request),
    Transfer(Transfer),
    Digest(Digest),
}

// ワイヤ上の外装(バージョンタグ付き)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireEnvelope
{
    pub wire: String, // WIRE_VERSION
    pub msg: WireMessage,
}

impl WireEnvelope
{
    pub fn new(msg: WireMessage) -> Self
    {
        Self { wire: WIRE_VERSION.to_string(), msg }
    }
}

// JSONエンコード(トランスポート共通のシリアライズ形)。
pub fn encode_message(msg: &WireMessage) -> String
{
    serde_json::to_string(&WireEnvelope::new(msg.clone()))
        .expect("wireメッセージのシリアライズに失敗")
}

// JSONデコード + バージョン確認。バージョン不一致・未知バリアントは Err
// (受信側は該当メッセージを無視する)。
pub fn decode_message(data: &str) -> Result<WireMessage, String>
{
    let env: WireEnvelope =
        serde_json::from_str(data).map_err(|e| format!("wireメッセージのパース失敗: {e}"))?;
    if env.wire != WIRE_VERSION
    {
        return Err(format!("非対応のwireバージョン: {}", env.wire));
    }
    Ok(env.msg)
}
