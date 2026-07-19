// 分散セマンティックキャッシュ — キャッシュ本体(SemanticCache)。
//
// S2.5 エントリ形式の型・正準化・ID算出は entry.rs に分離した
// (S3設計ノート §9 のモジュール分割)。本モジュールは:
//   - S2.5 load/verify の10手順(§6。順序厳守・失敗エントリは drop)
//   - 全複製ブルートフォース検索(全エントリの正規化済みEmbeddingとの
//     内積=コサイン類似度を総当たり計算。PoC規模ではO(n)で十分。
//     将来ANN等に差し替え可能。S3設計ノート §5)
//   - 登録(冪等マージ: 同一 entry_id は完全重複スキップ)と
//     question_key による版束ね(同一 question_key × 異 entry_id は
//     別版として併存。S2.5 §5.2, §6)
//   - shareable / tier / volatility 運用値の再導出
//     (derive_operative_state。送信者値を信頼しない。S2.5 §2, §6 手順8)
//   - ネット越し受信取り込み(S3設計ノート §3 受信側検証手順・§4 冪等マージ):
//     verify_envelope(手順3〜9)+ insert_verified(手順10)。単一ノードの
//     load() と S3 の ingest が同一の検証コードパスを共有する(§3「配送は
//     S2.5 の load/verify をネット越しに呼ぶだけ」)。手順2(組織PKI検証)は
//     配送層の責務であり sync.rs が policy::CertPolicy 経由で行う。
//     受信対象は .entry(core+署名)のみ。ピアの state.json は取り込まない
//     (S2.5 §13 補足: state はノードローカル。各ノードが手順8で自前導出)。
// に専念する。
//
// S3 でのシグネチャ変更: embedder/signer は &'a dyn 借用から Arc<dyn> 所有へ。
// デーモン(axum)とマルチノードのプロセス内テストが N 個の SemanticCache を
// スレッド間で共有するため(自己参照構造体を避ける)。検証・登録のロジック自体は
// 不変条件込みで S2.5 実装のまま。
use crate::embedder::Embedder;
use crate::entry::
{
    encode_core, entry_id, nfc, parse_core, question_key, CacheEntry, EntryEnvelope,
    ImmutableCore, MutableState, Provenance, Tier, Trust, SCHEMA_VER,
};
use crate::policy::TrustPolicy;
use crate::signer::Signer;
use crate::triples::{predicate_class, FactTriple, TripleDecomposition};
use crate::trust::prefer_candidate;
use crate::volatility::{finalize_volatility, share_gate, VolatilityAssessment};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chrono::Utc;
use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

// しきい値(設計メモ §1, §2 / Architecture §5.1):
// ローカル利用は0.8前後、共有想定は精度優先で0.9+
pub const LOCAL_THRESHOLD: f32 = 0.80;
pub const SHARED_THRESHOLD: f32 = 0.90;

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

// ネット越し受信取り込みの結果(S3設計ノート §4 冪等マージ)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestOutcome
{
    Added,     // 新規取り込み(grow-only set への追加)
    Duplicate, // 同一 entry_id 既存 → 冪等スキップ(core_bytes 同一なので衝突しない)
}

#[derive(Debug, Clone)]
pub struct IngestReport
{
    pub entry_id: String,
    pub question_key: String,
    pub shareable: bool, // 受信側で再導出した運用値(送信者主張ではない)
    pub outcome: IngestOutcome,
}

// 受信側検証の失敗理由(S3設計ノート §3 手順3〜6 / S2.5 §6 手順2〜5)。
// どの理由でも該当エントリを drop する(他エントリの処理は続行)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestError
{
    UnsupportedSchema(u16),                            // schema_ver 不一致(§7)
    Base64Decode,                                      // core_b64 復号失敗
    HashMismatch { expected: String, actual: String }, // ハッシュ照合失敗=改ざん検知
    BadSignature,                                      // 署名検証失敗=偽造防止
    MalformedCore,                                     // parse_core 失敗=形式不正
}

impl fmt::Display for IngestError
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result
    {
        match self
        {
            IngestError::UnsupportedSchema(v) => write!(f, "非対応 schema_ver={v}"),
            IngestError::Base64Decode => write!(f, "core_b64 復号失敗"),
            IngestError::HashMismatch { expected, actual } => write!(
                f,
                "ハッシュ不一致=改ざん検知 (expected={} actual={})",
                &expected[..16.min(expected.len())],
                &actual[..16.min(actual.len())]
            ),
            IngestError::BadSignature => write!(f, "署名不正=偽造防止"),
            IngestError::MalformedCore => write!(f, "core 形式不正"),
        }
    }
}

pub struct SemanticCache
{
    store_dir: PathBuf,
    embedder: Arc<dyn Embedder>,
    signer: Arc<dyn Signer>,
    threshold: f32,
    entries: Vec<CacheEntry>,
}

impl SemanticCache
{
    pub fn new(
        store_dir: PathBuf,
        embedder: Arc<dyn Embedder>,
        signer: Arc<dyn Signer>,
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

    // 全エントリの参照(digest 生成・テスト検査用。読み取り専用)。
    pub fn entries(&self) -> &[CacheEntry]
    {
        &self.entries
    }

    pub fn contains(&self, entry_id: &str) -> bool
    {
        self.entries.iter().any(|e| e.entry_id == entry_id)
    }

    pub fn get(&self, entry_id: &str) -> Option<&CacheEntry>
    {
        self.entries.iter().find(|e| e.entry_id == entry_id)
    }

    // Transfer(S3設計ノート §3)用に .entry エンベロープを再構成する。
    // core+署名のみを運び、mutable_state は含めない(受信側が再導出する)。
    pub fn envelope_for(&self, entry_id: &str) -> Option<EntryEnvelope>
    {
        self.get(entry_id).map(|e| EntryEnvelope
        {
            schema_ver: e.core.schema_ver,
            core_b64: B64.encode(&e.core_bytes),
            author_pub: e.author_pub.clone(),
            author_sig: e.author_sig.clone(),
        })
    }

    pub fn lookup(&self, question: &str) -> LookupResult<'_>
    {
        self.lookup_filtered(question, &|_| true)
    }

    // 検索(除外フィルタフック付き。trust 重みなし = 従来挙動)。
    // S4 実測ゲート既定(重み0)と完全に同じ順位になる後方互換ラッパ。
    pub fn lookup_filtered(
        &self,
        question: &str,
        include: &dyn Fn(&CacheEntry) -> bool,
    ) -> LookupResult<'_>
    {
        self.lookup_filtered_weighted(question, include, 0.0)
    }

    // 検索(除外フィルタフック+trust タイブレーク重み付き)。
    // include が false を返したエントリは候補から除外する。これは S3設計ノート
    // §8-4「grow-only 前提を検索に焼き込むな」の失効フィルタフックであり、
    // Phase1 では呼び出し側(sync.rs)が失効ポリシー(常にpass)+ TTL検索除外
    // (§7。物理削除はしない)をここに差し込む。Phase2 の revocation は
    // このフックの中身を差し替えるだけで載る。
    //
    // 複数版併存時の選好(S3設計ノート §4 / S4設計ノート §4・§9-2):
    // 類似度が同点の場合の選好は trust::prefer_candidate(純粋関数)に委譲する。
    //   - 主軸: created の新しい版(S3 §4 の従来選好そのまま)
    //   - trust_weight > 0(実測ゲート有効)のときのみ、created も同点の候補間で
    //     層1 trust(independent_agreement / supporting_versions)をタイブレーク
    //     として加味する。trust_weight = 0.0(既定)では trust 値がどうであれ
    //     従来と同一の順位になる(S4 §4 実測ゲート。層1は助言のみ=順位の
    //     タイブレーク以外の意思決定には一切使わない)。
    pub fn lookup_filtered_weighted(
        &self,
        question: &str,
        include: &dyn Fn(&CacheEntry) -> bool,
        trust_weight: f64,
    ) -> LookupResult<'_>
    {
        if self.entries.is_empty()
        {
            return LookupResult { entry: None, similarity: 0.0 };
        }
        let q = self.embedder.encode(question);
        // (index, similarity) の最良候補。閾値未満でも最良値は報告する(観測用)
        let mut best: Option<(usize, f32)> = None;
        for (i, e) in self.entries.iter().enumerate()
        {
            if !include(e)
            {
                continue; // 失効/TTL等の検索除外(物理削除はしない)
            }
            let sim = dot(&e.embedding, &q);
            let replace = match best
            {
                None => sim > 0.0,
                Some((bi, bs)) =>
                {
                    let b = &self.entries[bi];
                    sim > bs
                        || (sim == bs
                            && prefer_candidate(
                                &e.core.created,
                                e.state.trust.as_ref(),
                                &b.core.created,
                                b.state.trust.as_ref(),
                                trust_weight,
                            ))
                }
            };
            if replace
            {
                best = Some((i, sim));
            }
        }
        match best
        {
            Some((i, sim)) if sim >= self.threshold => LookupResult
            {
                entry: Some(&self.entries[i]),
                similarity: sim,
            },
            Some((_, sim)) => LookupResult { entry: None, similarity: sim },
            None => LookupResult { entry: None, similarity: 0.0 },
        }
    }

    // S1互換の縮約登録(L0判定のみの呼び出し元・テスト・ベンチ用)。
    // 事実トリプルなし・確信度は既定値で register へ委譲する。
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
    //     judge_entry の全段ANDゲート)をそのまま入れ、state.json にも保存する。
    //     reload 時は §6 手順9 に従い derive_operative_state の再導出値を基準に
    //     しつつ、shareable に限り disk 値を「上限(cap)」として AND 合成する
    //     (load() 手順9の【M-2 単調性保護】参照)。
    //     背景: derive_operative_state は answer 平文と agent を持たないため
    //     L2自己申告・fully_decomposed を適用できず、judge_entry より条件が
    //     真部分集合 = 緩くなりうる(再導出単独では reload で shareable が
    //     false→true に反転しうる非単調性がある。S2.5 §13 High-1)。
    //     S2 時点は shareable を消費する伝播経路が無く実害がなかったが、
    //     S3 で shareable が伝播ゲート(broadcast_announce /
    //     handle_entry_request / Digest / anti-entropy)に配線されたため、
    //     この反転を放置すると judge_entry が共有不可とした緩いエントリが
    //     reload を経て網に流れうる。よって「登録/取込時にそのノードが確定した
    //     shareable を state.json に保持し、reload 後の再導出はそれを下回る
    //     方向にのみ作用する」ことで保守的共有ゲート(疑わしきは共有しない)を
    //     維持する(脅威レビュー M-2 対応)。
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
        let id = entry_id(&core_bytes);

        // 重複排除(§6): 同一 entry_id は完全重複としてスキップ(冪等マージ)。
        // 同一 question_key かつ異なる entry_id は別版として併存させる(§5.2)。
        if let Some(i) = self.entries.iter().position(|e| e.entry_id == id)
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
            entry_id: id,
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

    // (旧 save_state は削除: S4 trust 再導出専用のヘルパーだったが、案Bで
    //  trust が非永続化(serde(skip)・起動時再導出)になり呼び出し元が消えた。
    //  state.json の書き込みは登録/取込時の save が担う)

    // ------------------------------------------------------------------
    // S4 層1: trust 再導出(S4設計ノート §3)
    // ------------------------------------------------------------------

    // 指定 question_key バンドル(同一 question_key × 異 entry_id の版集合。
    // S3 §4 複数版併存)の層1 trust をローカル再導出し、バンドル内全版の
    // mutable_state.trust へメモリ上でのみ格納する(save_state は呼ばない)。
    //
    //   - trust は非永続化(MutableState.trust は serde(skip)。案B):
    //     起動時に NodeService::new が recompute_trust_all で必ず再導出する
    //     ため、state.json へ書いてもロード時に読まれず上書きされる冗長 I/O
    //     にしかならない。よってここでは書き込まない。
    //
    //   - 入力はこのノードがローカル保持する版集合の facts のみ(S3 全複製方針。
    //     S4 §2(e))。送信者側 trust 値は一切参照しない(そもそも Transfer は
    //     trust を運ばない。S4 §3「各ノードがローカル算出」)。
    //   - 算出は policy(5点目の差し替え点)経由。Phase1 既定 =
    //     Layer1TrustPolicy(純粋関数 trust::compute_layer1_trust へ委譲)。
    //   - trust は署名対象外・entry_id 対象外の mutable_state 側であり、
    //     この更新は entry_id / author_sig に一切影響しない(S4 §7)。
    //   - 戻り値は算出された trust(バンドルが空 = 該当 question_key の版が
    //     1つも無い場合は None)。
    pub fn recompute_trust_for_bundle(
        &mut self,
        question_key: &str,
        policy: &dyn TrustPolicy,
    ) -> Option<Trust>
    {
        let indices: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.question_key == question_key)
            .map(|(i, _)| i)
            .collect();
        if indices.is_empty()
        {
            return None;
        }
        let trust = {
            let version_facts: Vec<&[FactTriple]> = indices
                .iter()
                .map(|&i| self.entries[i].core.facts.as_slice())
                .collect();
            policy.compute(&version_facts)
        };
        for &i in &indices
        {
            // メモリ上の更新のみ。trust は serde(skip) で state.json に
            // 含まれないため、save_state を呼んでもディスク内容は変わらない
            // (=冗長 I/O)。よってここでは永続化しない(案B)。
            self.entries[i].state.trust = Some(trust.clone());
        }
        Some(trust)
    }

    // 全 question_key バンドルの trust を再導出する(ロード直後の初期化用。
    // S4 §9-5「trust再導出は judge_entry/取込時の再導出と同時」の起動時版:
    // ディスク上の state.json に残る過去の trust 値も、現在ローカルに揃っている
    // 版集合から算出し直した値で上書きする = 常に自ノードの再導出値のみを保持)。
    pub fn recompute_trust_all(&mut self, policy: &dyn TrustPolicy)
    {
        let keys: BTreeSet<String> =
            self.entries.iter().map(|e| e.question_key.clone()).collect();
        for k in keys
        {
            self.recompute_trust_for_bundle(&k, policy);
        }
    }

    // ------------------------------------------------------------------
    // 受信側検証(単一ノード load とネット越し ingest の共有コードパス。
    // S2.5 §6 手順2〜8 = S3設計ノート §3 手順3〜9)
    // ------------------------------------------------------------------

    // エンベロープを検証し、検証済みの CacheEntry を構築する(取り込みはしない)。
    //   - expected_entry_id: ロード時はファイル名 <entry_id>、受信時は Announce の
    //     entry_id。None の場合は照合をスキップする(entry_id は core_bytes から
    //     再計算した値が常に正であり、None でも内容アドレス性は失われない)。
    //   - 手順(順序厳守): schema_ver → base64 復号 → ハッシュ照合(改ざん検知)
    //     → Signer::verify(偽造防止)→ parse_core(形式不正)→ question_key 再計算
    //     → embedding ローカル再計算 → derive_operative_state 再導出
    //     (送信者の shareable/tier/volatility を一切信頼しない)。
    //   - この署名検証コア(ハッシュ照合 + Signer::verify)はポリシーで差し替え
    //     不能なハードコードである(S3設計ノート §8-2: 差し替え可能なのは
    //     node_cert 検証=CertPolicy の側だけで、author_sig 検証は不変)。
    pub fn verify_envelope(
        &self,
        envelope: &EntryEnvelope,
        expected_entry_id: Option<&str>,
    ) -> Result<CacheEntry, IngestError>
    {
        // schema_ver 不一致は警告対象(§7。旧形式残存 = --keep 時を含む)
        if envelope.schema_ver != SCHEMA_VER
        {
            return Err(IngestError::UnsupportedSchema(envelope.schema_ver));
        }

        // 手順2(S2.5): base64 復号
        let core_bytes = B64
            .decode(&envelope.core_b64)
            .map_err(|_| IngestError::Base64Decode)?;

        // 手順3: hex(sha256(core_bytes)) == 期待ID か(改ざん検知)
        let actual_id = entry_id(&core_bytes);
        if let Some(expected) = expected_entry_id
        {
            if actual_id != expected
            {
                return Err(IngestError::HashMismatch
                {
                    expected: expected.to_string(),
                    actual: actual_id,
                });
            }
        }

        // 手順4: 署名検証(偽造防止)。ハッシュ照合とは役割が別(設計メモ §4)
        if !self.signer.verify(&envelope.author_pub, &envelope.author_sig, &core_bytes)
        {
            return Err(IngestError::BadSignature);
        }

        // 手順5: parse_core(形式不正なら drop)
        let core = parse_core(&core_bytes).ok_or(IngestError::MalformedCore)?;
        // core 内の schema_ver も照合(エンベロープ値との食い違い検出。§7)
        if core.schema_ver != SCHEMA_VER
        {
            return Err(IngestError::UnsupportedSchema(core.schema_ver));
        }

        // 手順6: question_key を再計算(保存値・送信者値があっても信頼しない)
        let qkey = question_key(&core.question_norm);

        // 手順7: embedding を自ノードの embedder で再計算(非保存。改良案C)
        let embedding = self.embedder.encode(&core.question_norm);

        // 手順8: shareable / tier / volatility 運用値を再導出(送信者値不信任)
        let derived = derive_operative_state(&core);
        let state = MutableState
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

        Ok(CacheEntry
        {
            entry_id: actual_id,
            question_key: qkey,
            core,
            core_bytes,
            author_pub: envelope.author_pub.clone(),
            author_sig: envelope.author_sig.clone(),
            state,
            embedding,
        })
    }

    // 検証済みエントリの取り込み(S3設計ノート §3 手順10 / §4 冪等マージ)。
    //   - 同一 entry_id 既存 → Duplicate(冪等スキップ。core_bytes 同一なので
    //     衝突しない。どの順序で同期しても収束する grow-only set)。
    //   - 同一 question_key × 異 entry_id は別版として併存(§4 複数版併存)。
    //   - 取り込み時にディスクへも保存する(.entry + 自ノード導出の .state.json)。
    pub fn insert_verified(&mut self, e: CacheEntry) -> IngestReport
    {
        if self.contains(&e.entry_id)
        {
            return IngestReport
            {
                entry_id: e.entry_id,
                question_key: e.question_key,
                shareable: e.state.shareable,
                outcome: IngestOutcome::Duplicate,
            };
        }
        let report = IngestReport
        {
            entry_id: e.entry_id.clone(),
            question_key: e.question_key.clone(),
            shareable: e.state.shareable,
            outcome: IngestOutcome::Added,
        };
        self.save(&e);
        self.entries.push(e);
        report
    }

    // 受信エンベロープの検証+取り込み(verify_envelope + insert_verified の合成)。
    // 注意: S3 の配送文脈では手順2(組織PKI検証=CertPolicy)を呼び出し側
    // (sync::NodeService::ingest_transfer)が先に行う。本メソッド自体は
    // PKI 文脈なしの取り込み口(単一ノード・テスト用)としても使える。
    pub fn ingest_envelope(
        &mut self,
        envelope: &EntryEnvelope,
        expected_entry_id: Option<&str>,
    ) -> Result<IngestReport, IngestError>
    {
        let e = self.verify_envelope(envelope, expected_entry_id)?;
        Ok(self.insert_verified(e))
    }

    // ロード(S2.5 §6 の10手順・順序厳守)。
    // 手順2〜8は verify_envelope(受信側検証と共有のコードパス)に委譲し、
    // 手順9(state.json の非運用値のみ採用)はディスク起点のロード固有処理として
    // ここで行う。どの手順で落ちても該当エントリを drop するのみで、
    // 他エントリの読込は続行する。
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

            // 手順2〜8: 受信側検証と共有の検証コードパス
            let mut entry = match self.verify_envelope(&envelope, Some(&expected_id))
            {
                Ok(e) => e,
                Err(err) =>
                {
                    println!("[cache] 検証失敗エントリをスキップ({err}): {file_name}");
                    continue;
                }
            };

            // 手順9: state.json があれば confidence/evidence/Phase2空slot を読む。
            //        tier/volatility の運用値は必ず手順8の再導出値を採用し、
            //        ディスク上の値で上書きしない。
            //        (state.json はノードローカル限定。ピアからは受信しない =
            //         S2.5 §13 補足の制約はこの分岐がロード専用であることで守られる)
            //
            // 【M-2 単調性保護(脅威レビュー対応)】shareable のみ、disk 値を
            // 「上限(cap)」として AND 合成する:
            //   operative shareable = 再導出値 AND disk.shareable
            //   - disk.shareable は登録時 judge_entry(全段ANDゲート)/ 取込時
            //     再導出でこのノード自身が確定させた値。derive_operative_state
            //     (answer="" のため L2自己申告・fully_decomposed 不可)は
            //     judge_entry より緩く、再導出単独だと reload で false→true に
            //     反転して S3 の伝播ゲート(供出/Digest/announce)へ漏れうる。
            //   - AND は下げる方向にのみ作用する: disk を信頼して共有可へ
            //     「緩める」ことは決してない(送信者値不信任の不変条件は維持。
            //     再導出が false なら disk が true でも false のまま)。
            //   - state.json 不在・破損時は登録時判定を確認できないため
            //     保守側に倒し shareable=false とする(疑わしきは共有しない)。
            //     core 自体は検証済みなのでローカル検索用には生かす。
            let state_path = self.store_dir.join(format!("{expected_id}.state.json"));
            let mut disk_cap: Option<bool> = None; // None = 登録時判定を確認できず
            if let Ok(sdata) = fs::read_to_string(&state_path)
            {
                match serde_json::from_str::<MutableState>(&sdata)
                {
                    Ok(disk) =>
                    {
                        entry.state.volatility_confidence = disk.volatility_confidence;
                        entry.state.volatility_evidence = disk.volatility_evidence;
                        // trust は serde(skip) で state.json に存在しない
                        // (導出状態。起動時に NodeService::new の
                        //  recompute_trust_all が再導出する。案B)ため復元しない。
                        entry.state.witness_sigs = disk.witness_sigs;
                        entry.state.anchor_proof = disk.anchor_proof;
                        entry.state.stake = disk.stake;
                        // disk.share_reason / disk.tier_operative /
                        // disk.volatility_class_operative は意図的に破棄(§6 手順9)。
                        // disk.shareable は cap としてのみ消費する(M-2)。
                        disk_cap = Some(disk.shareable);
                    }
                    Err(ex) =>
                    {
                        // state は可変・非署名なので、壊れていても core は生かし
                        // 再導出値で復元する(entry 本体の drop 理由にはしない)。
                        // ただし shareable は cap 不明のため保守側(false)になる。
                        println!("[cache] state.json パース失敗(再導出値で継続): {expected_id} ({ex})");
                    }
                }
            }
            match disk_cap
            {
                Some(true) =>
                {
                    // 登録時判定=共有可。再導出値(手順8)をそのまま採用
                    // (再導出が false ならもちろん false のまま = AND)。
                }
                Some(false) =>
                {
                    if entry.state.shareable
                    {
                        entry.state.shareable = false;
                        entry.state.share_reason =
                            "[再導出/M-2単調性保護] 登録時判定が共有不可のため再導出結果に関わらず共有保留".to_string();
                    }
                }
                None =>
                {
                    if entry.state.shareable
                    {
                        entry.state.shareable = false;
                        entry.state.share_reason =
                            "[再導出/M-2単調性保護] state.json 不在/破損で登録時判定を確認できないため共有保留".to_string();
                    }
                }
            }

            // 手順10: entries へ push
            self.entries.push(entry);
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
