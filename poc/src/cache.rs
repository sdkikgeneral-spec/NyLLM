// 分散セマンティックキャッシュ PoC — キャッシュ本体(S2.5 エントリ形式)。
//
// S2.5 再設計(docs/S2.5_エントリ形式設計.md)の要点:
//   - authoritative(署名・ハッシュ対象)= serde 非依存の長さ接頭辞バイナリ
//     (immutable_core。encode_core が生成する正準バイト列。§0, §3)。
//     serde JSON は一度もハッシュされないため、preserve_order の有無等の
//     serde 内部表現はハッシュ安定性に一切影響しない(§0)。
//   - on-disk = serde JSON エンベロープ:
//       <entry_id>.entry       … 不変(core は base64 の不透明ブロブ)
//       <entry_id>.state.json  … 可変(署名しない・各ノードが上書き)
//   - entry_id = hex(sha256(core_bytes)) = ファイル名 … 改ざん"検知"
//     author_sig = Signer::sign_bytes(core_bytes)      … 詐称"防止"
//     両者は同一バイト列を覆う(設計メモ §4 の分離原則を構造として保証。§4, §5)。
//   - embedding はどこにも保存しない(改良案C)。ロード時に自ノードの
//     embedder で再計算する(§6)。
//   - shareable / tier / volatility の運用値はロード時に必ず再導出し、
//     ディスク上の値(送信者の主張)を信頼しない(§2, §6 手順8)。
//   - core は answer 平文を保存しない(facts トリプルのみ。§1)。
//
// 検索: 全エントリの正規化済みEmbeddingとの内積(=コサイン類似度)を
//       総当たり計算。PoC規模ではO(n)で十分(将来ANN等に差し替え可能)。
use crate::embedder::Embedder;
use crate::signer::Signer;
use crate::triples::{predicate_class, FactTriple, TripleDecomposition};
use crate::volatility::{finalize_volatility, share_gate, VolatilityAssessment};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use unicode_normalization::UnicodeNormalization;

// しきい値(設計メモ §1, §2 / Architecture §5.1):
// ローカル利用は0.8前後、共有想定は精度優先で0.9+
pub const LOCAL_THRESHOLD: f32 = 0.80;
pub const SHARED_THRESHOLD: f32 = 0.90;

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
// 受信側は運用判断にこれらを信頼せず、必ず derive_operative_state で再導出する(§1)。
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trust {}

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust: Option<Trust>, // S4
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
    // 検索・重複排除・版束ねキー(§4)。本体バイナリの現行デモは entry_id 側の
    // 完全重複スキップのみ使うため未読(版束ねの利用とテストは別経路)。
    #[allow(dead_code)]
    pub question_key: String, // hex(sha256(qkey_bytes))
    pub core: ImmutableCore,
    pub core_bytes: Vec<u8>,  // authoritative な正準バイト列(保存はbase64)
    pub author_pub: String,
    pub author_sig: String,
    pub state: MutableState,
    pub embedding: Vec<f32>,  // ローカル再計算・非保存(改良案C)
}

// ディスク上のエンベロープ(非authoritative=serde自由。preserve_order無関係。§6)。
#[derive(Serialize, Deserialize)]
struct EntryEnvelope
{
    schema_ver: u16,
    core_b64: String,
    author_pub: String,
    author_sig: String,
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
fn push_lp_str(buf: &mut Vec<u8>, s: &str)
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

// ------------------------------------------------------------------
// 受信側再導出(S2.5 §6 手順8)
// ------------------------------------------------------------------

// 再導出された運用値。ロード時・登録時共通の導出結果コンテナ。
pub struct DerivedState
{
    pub volatility: VolatilityAssessment,
    pub shareable: bool,
    pub share_reason: String,
    pub tier_operative: Tier,
}

// 署名済み core の内容のみから shareable / volatility / tier の運用値を再導出する。
//
// ロード時には answer 平文が存在しない(core は facts のみ保存。§1)ため、
// pipeline::judge_entry(answer と Agent 実体が必要)をそのまま再実行できない。
// 代わりに「著者が署名で固定した内容 = question_norm + facts +
// initial_volatility_class」から再導出する。L2自己申告はロード時に再実行不能
// なので適用しない — 送信者が申告値を state に書いてきてもそれを信任するより、
// 署名済み core 内容から自ノードで導出する方が強い(§2「送信者の値は一切
// 信頼しない」の実装形)。
//
// Phase1 近似(コメント必須事項):
//   - TripleDecomposition の再構成: success = !facts.is_empty()、
//     fully_decomposed = success。元の回答の総文数はロード時に復元できないため
//     「facts が1件でもあれば完全分解扱い」とする近似。facts が空のエントリは
//     分解不能相当として共有不可側に倒れるので、近似の誤り方向は安全側。
//   - answer には "" を渡す(finalize_volatility ルール2の回答側走査は
//     ロード時には効かない。facts の述語クラス・目的語形状ガードが代替する)。
//   - declared には著者の initial_volatility_class を渡し、再導出クラスとの
//     不一致時に確信度が下がる既存挙動(§10.1 ルール4)をそのまま使う。
//   - tier_operative = core.initial_tier: ティア再分類器は Phase1 未実装
//     (設計レビュー §4.5 は将来課題)のため、著者の初期主張をそのまま
//     運用値に置く。Phase2 で再分類器が入ればここが差し替え点になる。
pub fn derive_operative_state(core: &ImmutableCore) -> DerivedState
{
    // facts から TripleDecomposition 相当を再構成する
    let mut unknown_predicates: Vec<String> = Vec::new();
    for t in &core.facts
    {
        if predicate_class(&t.p).is_none() && !unknown_predicates.contains(&t.p)
        {
            unknown_predicates.push(t.p.clone());
        }
    }
    let decomp = TripleDecomposition
    {
        success: !core.facts.is_empty(),
        fully_decomposed: !core.facts.is_empty(), // Phase1近似(上記コメント参照)
        triples: core.facts.clone(),
        unknown_predicates: unknown_predicates.clone(),
    };

    // 揮発性運用値(§10.1 の確定ルールを再適用。answer="" は上記コメント参照)
    let volatility =
        finalize_volatility(&core.question_norm, "", &decomp, &core.initial_volatility_class);

    // 共有可否の AND ゲート(保守的デフォルト維持。Architecture §7.1):
    //   L0語彙ゲート通過 AND 非volatile AND facts非空 AND 未知述語なし
    let gate = share_gate(&core.question_norm, &volatility.class);
    let (shareable, share_reason) = if !gate.shareable
    {
        (false, format!("[再導出/L0] {}", gate.reason))
    }
    else if volatility.class == "volatile"
    {
        // share_gate 内の volatile 判定と重複するが、AND 条件として明示する
        (false, "[再導出] volatility=volatile のためローカル短期TTLのみ".to_string())
    }
    else if core.facts.is_empty()
    {
        (
            false,
            "[再導出] facts が空(分解不能相当とみなし共有除外)".to_string(),
        )
    }
    else if !unknown_predicates.is_empty()
    {
        (
            false,
            format!(
                "[再導出] オントロジー未収録述語({})を含むため共有除外(allowlist)",
                unknown_predicates.join(", ")
            ),
        )
    }
    else
    {
        (
            true,
            "[再導出] 全ゲート通過(文脈自立 かつ 事実型 かつ 非volatile): 共有可".to_string(),
        )
    };

    DerivedState
    {
        volatility,
        shareable,
        share_reason,
        tier_operative: core.initial_tier,
    }
}

// ------------------------------------------------------------------
// キャッシュ本体
// ------------------------------------------------------------------

pub struct LookupResult<'a>
{
    pub entry: Option<&'a CacheEntry>,
    pub similarity: f32,
}

pub struct SemanticCache<'a>
{
    store_dir: PathBuf,
    embedder: &'a dyn Embedder,
    signer: &'a dyn Signer,
    threshold: f32,
    entries: Vec<CacheEntry>,
}

impl<'a> SemanticCache<'a>
{
    pub fn new(
        store_dir: PathBuf,
        embedder: &'a dyn Embedder,
        signer: &'a dyn Signer,
        threshold: f32,
    ) -> Self
    {
        fs::create_dir_all(&store_dir).expect("cache_store ディレクトリの作成に失敗");
        let mut cache = Self
        {
            store_dir,
            embedder,
            signer,
            threshold,
            entries: Vec::new(),
        };
        cache.load();
        cache
    }

    pub fn size(&self) -> usize
    {
        self.entries.len()
    }
    pub fn threshold(&self) -> f32
    {
        self.threshold
    }

    pub fn lookup(&self, question: &str) -> LookupResult<'_>
    {
        if self.entries.is_empty()
        {
            return LookupResult { entry: None, similarity: 0.0 };
        }
        let q = self.embedder.encode(question);
        let mut best_sim = 0.0f32;
        let mut best_idx: Option<usize> = None;
        for (i, e) in self.entries.iter().enumerate()
        {
            let sim = dot(&e.embedding, &q);
            if sim > best_sim
            {
                best_sim = sim;
                if sim >= self.threshold
                {
                    best_idx = Some(i);
                }
            }
        }
        if best_sim < self.threshold
        {
            LookupResult { entry: None, similarity: best_sim }
        }
        else
        {
            LookupResult
            {
                entry: best_idx.map(|i| &self.entries[i]),
                similarity: best_sim,
            }
        }
    }

    // S1互換の縮約登録(L0判定のみの呼び出し元・既存テスト・ベンチ用)。
    // 事実トリプルなし・確信度は既定値で register へ委譲する。
    // 本体バイナリからは未使用のため dead_code を明示的に許可(テストが使用)。
    #[allow(dead_code)]
    pub fn register_entry(
        &mut self,
        question: &str,
        answer: &str,
        volatility: &str,
        shareable: bool,
        share_reason: &str,
        agent_name: &str,
    ) -> &CacheEntry
    {
        let assessment = VolatilityAssessment
        {
            class: volatility.to_string(),
            confidence: 0.5, // L0のみの判定なので §10.1 ルール3相当の既定値
            evidence: vec!["l0_only".to_string()],
        };
        self.register(question, answer, &assessment, &[], shareable, share_reason, agent_name)
    }

    // 判定パイプライン(pipeline::judge_entry)の結果を添えた完全登録(S2経路)。
    //
    // S2.5 形式での組み立て:
    //   - question_norm = NFC + trim した question(§1 #2)
    //   - created = UTC RFC3339・秒精度・Z終端(§1 #7)
    //   - provenance = {agent_name, model(モック時 ""), embedder.name()}(§1 #4-6)
    //   - initial_tier = Tier::Low 既定。理由: ティア分類器は未実装で
    //     設計レビュー §4.5 のティア判定は将来課題。Phase1 は全登録を
    //     Tier-L 扱いとし、分類器実装時にここが差し替え点になる。
    //   - answer は保存しない(§1: core は facts のみ)。引数として受けるのは
    //     API形状の維持(将来の受信側再合成の入力になりうる)のため。
    //   - MutableState の operative 値は登録時点の判定結果(呼び出し元 =
    //     judge_entry の全段ANDゲート)をそのまま入れる。ただしこの値は
    //     このプロセスの生存中しか効力を持たない: reload 時は §6 手順9 に従い
    //     state.json 上の shareable/tier/volatility を破棄し、常に
    //     derive_operative_state の再導出値を採用する(disk の運用値を
    //     信頼しない = 送信者値不信任の不変条件)。
    //     derive_operative_state は answer 平文と agent を持たないため
    //     L2自己申告・fully_decomposed を適用できず、judge_entry より条件が
    //     真部分集合 = 緩くなりうる(reload で shareable が false→true に
    //     反転しうる非単調性がある)。これは、P2P 受信エントリでは L2 が
    //     攻撃者の自己申告にすぎず防御価値がなく、署名済み core
    //     (question_norm+facts)からの再導出の方が強い、という設計判断(§6)の
    //     帰結。Phase1 には shareable を消費する伝播経路が無いため実悪用は
    //     不可で、この非単調性は S3(伝播・tier配線)で扱う既知の Phase1 特性。
    pub fn register(
        &mut self,
        question: &str,
        _answer: &str, // 新形式では非保存(§1)。上記コメント参照
        volatility: &VolatilityAssessment,
        facts: &[FactTriple],
        shareable: bool,
        share_reason: &str,
        agent_name: &str,
    ) -> &CacheEntry
    {
        let question_norm = nfc(question).trim().to_string();
        let created = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let core = ImmutableCore
        {
            schema_ver: SCHEMA_VER,
            question_norm: question_norm.clone(),
            facts: facts.to_vec(),
            provenance: Provenance
            {
                agent: agent_name.to_string(),
                model: String::new(), // モック時 ""(実LLM経路が入ればモデルIDを渡す)
                embedder_model_id: self.embedder.name().to_string(),
            },
            created,
            initial_volatility_class: volatility.class.clone(),
            initial_tier: Tier::Low, // Phase1既定(上記コメント参照)
        };
        let core_bytes = encode_core(&core);
        let entry_id = sha256_hex(&core_bytes);

        // 重複排除(§6): 同一 entry_id は完全重複としてスキップ。
        // 同一 question_key かつ異なる entry_id は別版として併存させる(§5.2)。
        if let Some(i) = self.entries.iter().position(|e| e.entry_id == entry_id)
        {
            return &self.entries[i];
        }

        let author_sig = self.signer.sign_bytes(&core_bytes);
        let qkey = question_key(question);
        // embedding はメモリ保持のみ(非保存。改良案C)
        let embedding = self.embedder.encode(&core.question_norm);

        let state = MutableState
        {
            volatility_class_operative: volatility.class.clone(),
            volatility_confidence: volatility.confidence,
            volatility_evidence: volatility.evidence.clone(),
            shareable,
            share_reason: share_reason.to_string(),
            tier_operative: core.initial_tier,
            local_embedder_id: self.embedder.name().to_string(),
            trust: None,
            witness_sigs: Vec::new(),
            anchor_proof: None,
            stake: None,
        };

        let e = CacheEntry
        {
            entry_id,
            question_key: qkey,
            core,
            core_bytes,
            author_pub: self.signer.public_key_hex().to_string(),
            author_sig,
            state,
            embedding,
        };
        self.save(&e);
        self.entries.push(e);
        self.entries.last().unwrap()
    }

    // 保存(§6 レイアウト): <entry_id>.entry(不変)+ <entry_id>.state.json(可変)。
    // embedding はどちらにも保存しない(改良案C)。
    fn save(&self, e: &CacheEntry)
    {
        let envelope = EntryEnvelope
        {
            schema_ver: e.core.schema_ver,
            core_b64: B64.encode(&e.core_bytes),
            author_pub: e.author_pub.clone(),
            author_sig: e.author_sig.clone(),
        };
        let entry_path = self.store_dir.join(format!("{}.entry", e.entry_id));
        let data = serde_json::to_string_pretty(&envelope).expect("エンベロープのシリアライズに失敗");
        fs::write(entry_path, data).expect("キャッシュエントリの書き込みに失敗");

        let state_path = self.store_dir.join(format!("{}.state.json", e.entry_id));
        let state_data =
            serde_json::to_string_pretty(&e.state).expect("可変状態のシリアライズに失敗");
        fs::write(state_path, state_data).expect("可変状態の書き込みに失敗");
    }

    // ロード(S2.5 §6 の10手順・順序厳守)。
    // どの手順で落ちても該当エントリを drop するのみで、他エントリの読込は続行する。
    fn load(&mut self)
    {
        let mut files: Vec<PathBuf> = match fs::read_dir(&self.store_dir)
        {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().map(|ext| ext == "entry").unwrap_or(false))
                .collect(),
            Err(_) => Vec::new(),
        };
        files.sort();

        for f in files
        {
            let file_name = f.file_name().unwrap().to_string_lossy().to_string();
            // ファイル名 <entry_id>.entry の <entry_id> 部分
            let expected_id = f.file_stem().unwrap().to_string_lossy().to_string();

            // 手順1: .entry を serde でパースし core_b64 / author_pub / author_sig を取得
            let data = match fs::read_to_string(&f)
            {
                Ok(d) => d,
                Err(ex) =>
                {
                    println!("[cache] 破損エントリをスキップ: {file_name} ({ex})");
                    continue;
                }
            };
            let envelope: EntryEnvelope = match serde_json::from_str(&data)
            {
                Ok(v) => v,
                Err(ex) =>
                {
                    println!("[cache] 破損エントリをスキップ: {file_name} ({ex})");
                    continue;
                }
            };
            // schema_ver 不一致は警告して drop(§7。旧形式残存 = --keep 時を含む)
            if envelope.schema_ver != SCHEMA_VER
            {
                println!(
                    "[cache] 非対応 schema_ver={} をスキップ: {file_name}",
                    envelope.schema_ver
                );
                continue;
            }

            // 手順2: base64 復号
            let core_bytes = match B64.decode(&envelope.core_b64)
            {
                Ok(b) => b,
                Err(_) =>
                {
                    println!("[cache] core_b64 復号失敗エントリをスキップ: {file_name}");
                    continue;
                }
            };

            // 手順3: sha256_hex(core_bytes) == ファイル名 か(改ざん検知)
            if sha256_hex(&core_bytes) != expected_id
            {
                println!("[cache] 検証失敗エントリをスキップ(ハッシュ不一致=改ざん検知): {file_name}");
                continue;
            }

            // 手順4: 署名検証(偽造防止)。ハッシュ照合とは役割が別(設計メモ §4)
            if !self.signer.verify(&envelope.author_pub, &envelope.author_sig, &core_bytes)
            {
                println!("[cache] 検証失敗エントリをスキップ(署名不正=偽造防止): {file_name}");
                continue;
            }

            // 手順5: parse_core(形式不正なら drop)
            let core = match parse_core(&core_bytes)
            {
                Some(c) => c,
                None =>
                {
                    println!("[cache] 形式不正エントリをスキップ: {file_name}");
                    continue;
                }
            };
            // core 内の schema_ver も照合(エンベロープ値との食い違い検出。§7)
            if core.schema_ver != SCHEMA_VER
            {
                println!(
                    "[cache] 非対応 schema_ver={}(core側)をスキップ: {file_name}",
                    core.schema_ver
                );
                continue;
            }

            // 手順6: question_key を再計算(保存値があっても信頼しない)
            let qkey = question_key(&core.question_norm);

            // 手順7: embedding を自ノードの embedder で再計算(非保存。改良案C)
            let embedding = self.embedder.encode(&core.question_norm);

            // 手順8: shareable / tier / volatility 運用値を再導出
            //        (送信者値 = state.json の値を信頼しない)
            let derived = derive_operative_state(&core);
            let mut state = MutableState
            {
                volatility_class_operative: derived.volatility.class.clone(),
                volatility_confidence: derived.volatility.confidence,
                volatility_evidence: derived.volatility.evidence.clone(),
                shareable: derived.shareable,
                share_reason: derived.share_reason.clone(),
                tier_operative: derived.tier_operative,
                local_embedder_id: self.embedder.name().to_string(),
                trust: None,
                witness_sigs: Vec::new(),
                anchor_proof: None,
                stake: None,
            };

            // 手順9: state.json があれば confidence/evidence/Phase2空slot を読む。
            //        ただし shareable/tier/volatility の運用値は必ず手順8の
            //        再導出値を採用し、ディスク上の値で上書きしない。
            let state_path = self.store_dir.join(format!("{expected_id}.state.json"));
            if let Ok(sdata) = fs::read_to_string(&state_path)
            {
                match serde_json::from_str::<MutableState>(&sdata)
                {
                    Ok(disk) =>
                    {
                        state.volatility_confidence = disk.volatility_confidence;
                        state.volatility_evidence = disk.volatility_evidence;
                        state.trust = disk.trust;
                        state.witness_sigs = disk.witness_sigs;
                        state.anchor_proof = disk.anchor_proof;
                        state.stake = disk.stake;
                        // disk.shareable / disk.share_reason / disk.tier_operative /
                        // disk.volatility_class_operative は意図的に破棄(§6 手順9)
                    }
                    Err(ex) =>
                    {
                        // state は可変・非署名なので、壊れていても core は生かし
                        // 再導出値のみで復元する(entry 本体の drop 理由にはしない)
                        println!("[cache] state.json パース失敗(再導出値で継続): {expected_id} ({ex})");
                    }
                }
            }

            // 手順10: entries へ push
            self.entries.push(CacheEntry
            {
                entry_id: expected_id,
                question_key: qkey,
                core,
                core_bytes,
                author_pub: envelope.author_pub,
                author_sig: envelope.author_sig,
                state,
                embedding,
            });
        }
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32
{
    if a.len() != b.len()
    {
        return 0.0;
    }
    let mut s = 0.0f64;
    for i in 0..a.len()
    {
        s += a[i] as f64 * b[i] as f64;
    }
    s as f32
}

pub fn sha256_hex(data: &[u8]) -> String
{
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}
