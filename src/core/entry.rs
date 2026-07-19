// S2.5 エントリ形式(docs/S2.5_エントリ形式設計.md)— 型・正準化・ID算出。
//
// poc では cache.rs に統合されていた部分を、S3設計ノート §9 のモジュール分割に
// 従い entry.rs として分離したもの。cache.rs(SemanticCache)と将来の wire.rs
// (Transfer エンベロープ)が本モジュールを共有する。
//
// S2.5 再設計の要点:
//   - authoritative(署名・ハッシュ対象)= serde 非依存の長さ接頭辞バイナリ
//     (immutable_core。encode_core が生成する正準バイト列。§0, §3)。
//     serde JSON は一度もハッシュされないため、preserve_order の有無等の
//     serde 内部表現はハッシュ安定性に一切影響しない(§0)。
//   - on-disk = serde JSON エンベロープ(EntryEnvelope。非authoritative):
//       <entry_id>.entry       … 不変(core は base64 の不透明ブロブ)
//       <entry_id>.state.json  … 可変(署名しない・各ノードが上書き)
//   - entry_id = hex(sha256(core_bytes)) = ファイル名 … 改ざん"検知"
//     author_sig = Signer::sign_bytes(core_bytes)      … 詐称"防止"
//     両者は同一バイト列を覆う(設計メモ §4 の分離原則を構造として保証。§4, §5)。
//   - core は answer 平文を保存しない(facts トリプルのみ。§1)。

use crate::triples::FactTriple;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

// スキーマ版(S2.5 §1 #1)。不一致のエントリはロード時に警告して drop する(§7)。
pub const SCHEMA_VER: u16 = 1;

// ドメイン分離タグ(S2.5 §11 ドメインタグ登録簿)。
// entry_id/author_sig の対象と question_key の対象を別タグ配下に置き、
// 将来の witness/revocation 署名との cross-protocol 流用を構造的に防ぐ。
// 注: 設計ノート §3 は entry タグを「16バイト固定」と記すが、ASCII 文字列
// "nyllm/entry/v1\n" の実バイト長は15。文字列リテラル側を正とする
// (ゴールデンテストはこのバイト列でピン留めされる)。
const ENTRY_DOMAIN_TAG: &[u8] = b"nyllm/entry/v1\n";
const QKEY_DOMAIN_TAG: &[u8] = b"nyllm/qkey/v1\n";

// ------------------------------------------------------------------
// データモデル(S2.5 §8 Rust型スケッチを起点)
// ------------------------------------------------------------------

// リスクティア(設計レビュー §4.5 幻覚パリティ2ティア)。u8正準表現: Low=0 / High=1。
// レイヤ中立(Public専用ではない。§0.1): 社内 Phase1 でも医療・個人情報は
// Tier-H 相当のタグを要する。serde 導出は state.json の表示用のみで、
// 正準バイト列側は to_u8/from_u8 の固定対応を使う。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tier
{
    Low,  // Tier-L: パリティ適用領域
    High, // Tier-H: パリティ禁止・別機構へ
}

impl Tier
{
    pub fn to_u8(self) -> u8
    {
        match self
        {
            Tier::Low => 0,
            Tier::High => 1,
        }
    }

    pub fn from_u8(v: u8) -> Option<Tier>
    {
        match v
        {
            0 => Some(Tier::Low),
            1 => Some(Tier::High),
            _ => None, // 未知値は形式不正として drop(§6 手順5)
        }
    }
}

// 出所メタ(agent を署名対象化 = 設計レビュー §2.2 弱点1への対応)。
#[derive(Debug, Clone, PartialEq)]
pub struct Provenance
{
    pub agent: String,
    pub model: String,             // モック時は ""
    pub embedder_model_id: String, // 著者が使ったembedder識別子(情報用。互換性ゲートには使わない)
}

// 不変コア(署名・ハッシュ対象。作成後不変。浮動小数を1つも含まない。§1)。
// 正準バイト列は encode_core() が serde 非依存で生成し、entry_id と author_sig は
// そのバイト列に対して計算する(改良案A)。
// initial_volatility_class / initial_tier は「真として信頼させる」ためではなく、
// 著者の主張を固定して事後説明責任(スラッシング根拠)を成立させるために署名する。
// 受信側は運用判断にこれらを信頼せず、必ず cache::derive_operative_state で再導出する(§1)。
#[derive(Debug, Clone, PartialEq)]
pub struct ImmutableCore
{
    pub schema_ver: u16,
    pub question_norm: String,            // NFC正規化・trim済み
    pub facts: Vec<FactTriple>,           // triples.rs の型を再利用。decompose の決定的順序を保持
    pub provenance: Provenance,
    pub created: String,                  // UTC RFC3339・秒精度・Z終端
    pub initial_volatility_class: String, // 著者の初期主張(運用値はMutableState側)
    pub initial_tier: Tier,               // 著者の初期リスク分類
}

// --- Phase2 空スロット型(S2.5 §8, §10-8) ---
// フィールドを持たない空 struct として今宣言し、中身は Phase2 で定義する
// (過剰設計回避=「動く的撃ち」の回避)。今確定するのは「可変状態側・
// 署名対象外」という配置と署名境界のみ。Phase2 で中身を埋めても
// entry_id は不変のまま「追加」で完結する(§0.1)。

// S4 層1(エントリ内在信頼度)。S4設計ノート §1 が S2.5 §10-8 の空宣言原則の
// 明示的な例外として Phase1 で先行定義する2フィールドのみを持つ
// (Architecture §8冒頭[補完]・Roadmap §0対応表S4行が既に認めていた例外の具体化)。
//
//   - mutable_state 側(署名対象外・entry_id 対象外)の助言値であり、値が
//     どう変わっても entry_id / author_sig には一切影響しない(§7 署名境界不変)。
//   - 各ノードが trust::compute_layer1_trust でローカル再導出した値のみを
//     格納する。送信者側の trust 値は一切参照しない(そもそも Transfer =
//     EntryEnvelope は core+署名のみで trust を運ばない。§3・§6)。
//   - 層2/層3 のフィールド(author_reputation / revoked)はここに定義しない
//     (Phase2 のまま。載る場所=本構造体内の未定義フィールドとして確保済み。§7)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trust
{
    // 同一 question_key の版集合における事実トリプルの一致率(0..1)。
    // 案A: 正規化トリプル集合の版ペア間 Jaccard 平均(S4 §3・§9-1)。
    #[serde(default)]
    pub independent_agreement: f64,
    // 一致率計算の対象になった版数(facts 分解成功版数。S4 §3)。
    #[serde(default)]
    pub supporting_versions: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WitnessSig {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnchorProof {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stake {}

// 可変状態(署名しない・entry_id に含めない・各ノードが導出/更新する。§2)。
// 送信者の値は信頼せず、shareable/tier/volatility 運用値はロード時に再導出する。
// serde は state.json の保存・読込にのみ使う(ハッシュ対象外なので自由)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutableState
{
    pub volatility_class_operative: String,
    pub volatility_confidence: f32,
    pub volatility_evidence: Vec<String>,
    pub shareable: bool,
    pub share_reason: String,
    pub tier_operative: Tier,
    pub local_embedder_id: String, // このノードが索引に使った embedder(変更時は全再embedding)
    // --- Phase2 空スロット(型のみ確保。Phase1では None/空) ---
    // trust は導出状態: 起動時に NodeService::new が recompute_trust_all で
    // ローカル版集合から必ず再導出するため、state.json へ永続化しない
    // (書いてもロード時に読まれず上書きされる冗長 I/O にしかならない。案B)。
    // 署名対象外・entry_id 対象外・助言のみ(既定重み0)なので非永続化は
    // 不変条件・脅威モデルに影響しない。
    #[serde(skip)]
    pub trust: Option<Trust>, // S4(メモリのみ保持・ロード時再導出)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub witness_sigs: Vec<WitnessSig>, // S3(社内は共通時計で代替=空)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_proof: Option<AnchorProof>, // S3(設計レビュー §4.2)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stake: Option<Stake>, // S4(設計レビュー §4.3)
}

// インメモリのエントリ(コア+コアバイト列+署名+可変状態+ローカルembedding)。
pub struct CacheEntry
{
    pub entry_id: String,     // hex(sha256(core_bytes))
    pub question_key: String, // hex(sha256(qkey_bytes))。検索・重複排除・版束ねキー(§4)
    pub core: ImmutableCore,
    pub core_bytes: Vec<u8>,  // authoritative な正準バイト列(保存はbase64)
    pub author_pub: String,
    pub author_sig: String,
    pub state: MutableState,
    pub embedding: Vec<f32>,  // ローカル再計算・非保存(改良案C)
}

// ディスク上のエンベロープ(非authoritative=serde自由。preserve_order無関係。§6)。
// S3 の Transfer(ノード間配送)もこのエンベロープをそのまま運ぶ(S3設計ノート §3。
// core+署名のみ = mutable_state は運ばない)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntryEnvelope
{
    pub schema_ver: u16,
    pub core_b64: String,
    pub author_pub: String,
    pub author_sig: String,
}

// ------------------------------------------------------------------
// 正準化(S2.5 §3 長さ接頭辞バイナリ。serde 非依存)
// ------------------------------------------------------------------

// Unicode NFC 正規化(§3: nfc(s))。
pub fn nfc(s: &str) -> String
{
    s.nfc().collect()
}

// lp_str(s) = u32 BE(bytelen(nfc(s))) || nfc(s) の UTF-8 バイト列(§3)。
// 長さ区切りのためエスケープという概念自体が存在しない。
// pub(crate): S3 node.rs の node_cert 署名対象バイト列(nyllm/node/v1 タグ配下)も
// 同じ lp_str 正準化を共有する(entry タグ配下の形式には影響しない)。
pub(crate) fn push_lp_str(buf: &mut Vec<u8>, s: &str)
{
    let n = nfc(s);
    buf.extend_from_slice(&(n.len() as u32).to_be_bytes());
    buf.extend_from_slice(n.as_bytes());
}

// immutable_core → 正準バイト列(§3。フィールド固定順・キーソートなし)。
// 順序: domain_tag → u16(schema_ver) → lp_str(question_norm)
//     → u32(facts数) + 各 fact lp_str(s) lp_str(p) lp_str(o)
//     → lp_str(agent) → lp_str(model) → lp_str(embedder_model_id)
//     → lp_str(created) → lp_str(initial_volatility_class) → u8(initial_tier)
// 数値は big-endian 固定幅。浮動小数点数は core に存在しない(§1)。
pub fn encode_core(core: &ImmutableCore) -> Vec<u8>
{
    let mut buf: Vec<u8> = Vec::new();
    buf.extend_from_slice(ENTRY_DOMAIN_TAG); // 長さ接頭辞なしの固定タグ
    buf.extend_from_slice(&core.schema_ver.to_be_bytes());
    push_lp_str(&mut buf, &core.question_norm);
    buf.extend_from_slice(&(core.facts.len() as u32).to_be_bytes());
    for t in &core.facts
    {
        push_lp_str(&mut buf, &t.s);
        push_lp_str(&mut buf, &t.p);
        push_lp_str(&mut buf, &t.o);
    }
    push_lp_str(&mut buf, &core.provenance.agent);
    push_lp_str(&mut buf, &core.provenance.model);
    push_lp_str(&mut buf, &core.provenance.embedder_model_id);
    push_lp_str(&mut buf, &core.created);
    push_lp_str(&mut buf, &core.initial_volatility_class);
    buf.push(core.initial_tier.to_u8());
    buf
}

// parse_core 用の読み取りカーソル。途中終端・長さ超過はすべて None に落とす。
struct Cursor<'a>
{
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a>
{
    fn take(&mut self, n: usize) -> Option<&'a [u8]>
    {
        // pos + n のオーバーフローを避けるため残量側で比較する
        if n > self.data.len() - self.pos
        {
            return None;
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Some(s)
    }

    fn read_u8(&mut self) -> Option<u8>
    {
        self.take(1).map(|b| b[0])
    }

    fn read_u16(&mut self) -> Option<u16>
    {
        self.take(2).map(|b| u16::from_be_bytes([b[0], b[1]]))
    }

    fn read_u32(&mut self) -> Option<u32>
    {
        self.take(4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    // lp_str の逆(§3)。UTF-8 不正は None。
    // 加えて NFC でない文字列も棄却する: encode_core は NFC しか生成しないため、
    // 非NFCを含む core_bytes は「正準形式で生成されていない」形式不正であり、
    // これを弾くことで encode_core(parse_core(b)) == b の round-trip 一意性が保たれる。
    fn read_lp_str(&mut self) -> Option<String>
    {
        let len = self.read_u32()? as usize;
        let bytes = self.take(len)?;
        let s = std::str::from_utf8(bytes).ok()?;
        if !unicode_normalization::is_nfc(s)
        {
            return None;
        }
        Some(s.to_string())
    }

    fn finished(&self) -> bool
    {
        self.pos == self.data.len()
    }
}

// 正準バイト列 → ImmutableCore(encode_core の逆。§6 手順5)。
// 長さ超過・途中終端・末尾の余剰バイト・domain_tag不一致・UTF-8不正・
// 未知 tier 値はすべて None(呼び出し側が形式不正として drop する)。
pub fn parse_core(bytes: &[u8]) -> Option<ImmutableCore>
{
    let mut cur = Cursor { data: bytes, pos: 0 };
    if cur.take(ENTRY_DOMAIN_TAG.len())? != ENTRY_DOMAIN_TAG
    {
        return None;
    }
    let schema_ver = cur.read_u16()?;
    let question_norm = cur.read_lp_str()?;
    let fact_count = cur.read_u32()?;
    let mut facts: Vec<FactTriple> = Vec::new();
    for _ in 0..fact_count
    {
        // 個数接頭辞は信頼せず1件ずつ読む(偽の巨大countによる事前確保をしない)。
        // バイト列が尽きれば read_lp_str が None を返し全体が形式不正になる。
        let s = cur.read_lp_str()?;
        let p = cur.read_lp_str()?;
        let o = cur.read_lp_str()?;
        facts.push(FactTriple { s, p, o });
    }
    let agent = cur.read_lp_str()?;
    let model = cur.read_lp_str()?;
    let embedder_model_id = cur.read_lp_str()?;
    let created = cur.read_lp_str()?;
    let initial_volatility_class = cur.read_lp_str()?;
    let initial_tier = Tier::from_u8(cur.read_u8()?)?;
    if !cur.finished()
    {
        // 末尾に余剰バイトが残る = 形式不正(§6 手順5)
        return None;
    }
    Some(ImmutableCore
    {
        schema_ver,
        question_norm,
        facts,
        provenance: Provenance { agent, model, embedder_model_id },
        created,
        initial_volatility_class,
        initial_tier,
    })
}

// ------------------------------------------------------------------
// ID算出(S2.5 §4)
// ------------------------------------------------------------------

// entry_id = hex(sha256(core_bytes))(§4)。内容アドレス(回答インスタンスID)であり、
// オンディスクのファイル名 <entry_id>.entry でもある。
pub fn entry_id(core_bytes: &[u8]) -> String
{
    sha256_hex(core_bytes)
}

// fold(q) = NFC → 小文字化 → 前後trim → 連続空白を単一空白へ畳む(§4)。
pub fn fold(q: &str) -> String
{
    // split_whitespace が trim と連続空白の畳み込みを兼ねる
    nfc(q)
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

// question_key = hex(sha256("nyllm/qkey/v1\n" || lp_str(fold(question))))(§4)。
// 検索・重複排除・版束ね用の content-based キー。言い換えはクラスタされない
// (PoC暫定・意図的制約。意味クラスタは検索層=embedding近傍検索に担わせ、
//  identity層と search層を混ぜない。§4)。
pub fn question_key(question: &str) -> String
{
    let mut buf: Vec<u8> = QKEY_DOMAIN_TAG.to_vec();
    push_lp_str(&mut buf, &fold(question));
    sha256_hex(&buf)
}

pub fn sha256_hex(data: &[u8]) -> String
{
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}
