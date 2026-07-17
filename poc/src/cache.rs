// 分散セマンティックキャッシュ PoC — キャッシュ本体。
//
// エントリのデータモデル(設計メモ §4 スキーマの縮小版):
//   entry_id (=ファイル名) : sha256(署名対象ペイロード) … 改ざん"検知"用
//   author_sig             : 署名(既定はダミーMAC、feature="ed25519"でEd25519)
//   witness_sigs           : 単一ノードPoCのため省略
//
// 検索: 全エントリの正規化済みEmbeddingとの内積(=コサイン類似度)を
//       総当たり計算。PoC規模ではO(n)で十分(将来ANN等に差し替え可能)。
use crate::embedder::Embedder;
use crate::signer::Signer;
use crate::triples::FactTriple;
use crate::volatility::VolatilityAssessment;
use chrono::Local;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;

// しきい値(設計メモ §1, §2): ローカル利用は0.8前後、共有想定は精度優先で0.9+
pub const LOCAL_THRESHOLD: f32 = 0.80;
pub const SHARED_THRESHOLD: f32 = 0.90;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub question: String,
    // 平文回答。Architecture §6 の実装specは「トリプルのみ保存・受信側再合成」
    // だが、PoCは再合成未実装のため平文も併存させる(意図的縮約)
    pub answer: String,
    pub embedding: Vec<f32>, // 正規化済み
    pub created: String, // ISO8601(claimed_date 相当・平文)
    pub volatility: String, // permanent | slow | volatile(確定クラス。署名対象)
    // 確信度・判定根拠(Architecture §10: 揮発性タグは確率的推定であり
    // 再評価で更新されうるため署名対象に含めない)
    pub volatility_confidence: f32,
    pub volatility_evidence: Vec<String>,
    // 事実トリプル(§6 facts。著者の主張内容そのものなので署名対象)
    pub facts: Vec<FactTriple>,
    pub shareable: bool,
    pub share_reason: String,
    pub agent: String,
    pub author_pub: String,
    pub author_sig: String,
    pub entry_id: String, // sha256(signed_payload) = content hash
}

impl CacheEntry {
    // 署名対象 = 質問 + 回答 + 日付 + 揮発性クラス + 事実トリプル(キー順ソートで正規化)
    //
    // 脅威レビュー Medium-4 対応の注記: 本PoCの署名対象は上記
    // question + answer + created + volatility(class) + facts であり、
    // Architecture §6 が定める provenance(agent / モデル情報)はまだ署名対象に
    // 含めていない(agent フィールドは平文保存のみ)。これはPoCの意図的縮約で
    // あり、§6 完全準拠(provenance の署名対象化)と受信側での再判定は
    // S3 着手前に対応予定。
    //
    // 不変条件: entry_id = sha256(この文字列)。署名対象を変えたら verify() 側と
    // 必ず同時に整合させること(verify() は本メソッド経由で再計算するため、
    // ここを変えれば id 計算と検証は自動的に揃うが、既存エントリは全て
    // ハッシュ不一致で無効化される点に注意)。
    // volatility_confidence / volatility_evidence は §10 の再評価で更新される
    // 可変推定値のため意図的に署名対象外(上記フィールドコメント参照)。
    pub fn signed_payload(&self) -> String {
        // serde_json::Map は既定(preserve_order未使用)でキーをソートして保持する。
        // facts は Vec の並びを保持する(分解は決定的なので順序も正準)
        let facts: Vec<serde_json::Value> = self
            .facts
            .iter()
            .map(|t| json!({ "s": t.s, "p": t.p, "o": t.o }))
            .collect();
        let j = json!({
            "question": self.question,
            "answer": self.answer,
            "created": self.created,
            "volatility": self.volatility,
            "facts": facts,
        });
        j.to_string()
    }
}

pub struct LookupResult<'a> {
    pub entry: Option<&'a CacheEntry>,
    pub similarity: f32,
}

pub struct SemanticCache<'a> {
    store_dir: PathBuf,
    embedder: &'a dyn Embedder,
    signer: &'a dyn Signer,
    threshold: f32,
    entries: Vec<CacheEntry>,
}

impl<'a> SemanticCache<'a> {
    pub fn new(
        store_dir: PathBuf,
        embedder: &'a dyn Embedder,
        signer: &'a dyn Signer,
        threshold: f32,
    ) -> Self {
        fs::create_dir_all(&store_dir).expect("cache_store ディレクトリの作成に失敗");
        let mut cache = Self {
            store_dir,
            embedder,
            signer,
            threshold,
            entries: Vec::new(),
        };
        cache.load();
        cache
    }

    pub fn size(&self) -> usize {
        self.entries.len()
    }
    pub fn threshold(&self) -> f32 {
        self.threshold
    }

    pub fn lookup(&self, question: &str) -> LookupResult<'_> {
        if self.entries.is_empty() {
            return LookupResult {
                entry: None,
                similarity: 0.0,
            };
        }
        let q = self.embedder.encode(question);
        let mut best_sim = 0.0f32;
        let mut best_idx: Option<usize> = None;
        for (i, e) in self.entries.iter().enumerate() {
            let sim = dot(&e.embedding, &q);
            if sim > best_sim {
                best_sim = sim;
                if sim >= self.threshold {
                    best_idx = Some(i);
                }
            }
        }
        if best_sim < self.threshold {
            LookupResult {
                entry: None,
                similarity: best_sim,
            }
        } else {
            LookupResult {
                entry: best_idx.map(|i| &self.entries[i]),
                similarity: best_sim,
            }
        }
    }

    // S1互換の縮約登録(L0判定のみの呼び出し元・既存テスト・ベンチ用)。
    // 事実トリプルなし・確信度は既定値で register_judged_entry へ委譲する。
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
    ) -> &CacheEntry {
        let assessment = VolatilityAssessment
        {
            class: volatility.to_string(),
            confidence: 0.5, // L0のみの判定なので §10.1 ルール3相当の既定値
            evidence: vec!["l0_only".to_string()],
        };
        self.register_judged_entry(question, answer, &assessment, &[], shareable, share_reason, agent_name)
    }

    // 判定パイプライン(pipeline::judge_entry)の結果を添えた完全登録(S2経路)。
    pub fn register_judged_entry(
        &mut self,
        question: &str,
        answer: &str,
        volatility: &VolatilityAssessment,
        facts: &[FactTriple],
        shareable: bool,
        share_reason: &str,
        agent_name: &str,
    ) -> &CacheEntry {
        let embedding = self.embedder.encode(question);
        let created = Local::now().format("%Y-%m-%dT%H:%M:%S").to_string();
        let mut e = CacheEntry {
            question: question.to_string(),
            answer: answer.to_string(),
            embedding,
            created,
            volatility: volatility.class.clone(),
            volatility_confidence: volatility.confidence,
            volatility_evidence: volatility.evidence.clone(),
            facts: facts.to_vec(),
            shareable,
            share_reason: share_reason.to_string(),
            agent: agent_name.to_string(),
            author_pub: self.signer.public_key_hex().to_string(),
            author_sig: String::new(),
            entry_id: String::new(),
        };
        let payload = e.signed_payload();
        e.author_sig = self.signer.sign_hex(&payload);
        e.entry_id = sha256_hex(payload.as_bytes());

        self.save(&e);
        self.entries.push(e);
        self.entries.last().unwrap()
    }

    fn save(&self, e: &CacheEntry) {
        let path = self.store_dir.join(format!("{}.json", e.entry_id));
        let data = serde_json::to_string_pretty(e).expect("エントリのシリアライズに失敗");
        fs::write(path, data).expect("キャッシュエントリの書き込みに失敗");
    }

    fn load(&mut self) {
        let mut files: Vec<PathBuf> = match fs::read_dir(&self.store_dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().map(|ext| ext == "json").unwrap_or(false))
                .collect(),
            Err(_) => Vec::new(),
        };
        files.sort();

        for f in files {
            let file_name = f.file_name().unwrap().to_string_lossy().to_string();
            let data = match fs::read_to_string(&f) {
                Ok(d) => d,
                Err(ex) => {
                    println!("[cache] 破損エントリをスキップ: {file_name} ({ex})");
                    continue;
                }
            };
            let e: CacheEntry = match serde_json::from_str(&data) {
                Ok(e) => e,
                Err(ex) => {
                    println!("[cache] 破損エントリをスキップ: {file_name} ({ex})");
                    continue;
                }
            };
            let expected_id = f
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .to_string();
            if !self.verify(&e, &expected_id) {
                println!("[cache] 検証失敗エントリをスキップ: {file_name}");
                continue;
            }
            self.entries.push(e);
        }
    }

    // 改ざん検知(content hash) + 署名検証(author_sig)
    fn verify(&self, e: &CacheEntry, expected_id: &str) -> bool {
        let payload = e.signed_payload();
        let h = sha256_hex(payload.as_bytes());
        if e.entry_id != h || expected_id != h {
            return false;
        }
        self.signer.verify(&e.author_pub, &e.author_sig, &payload)
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let mut s = 0.0f64;
    for i in 0..a.len() {
        s += a[i] as f64 * b[i] as f64;
    }
    s as f32
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}
