// Winny型 Semantic Cache PoC — キャッシュ本体。
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
    pub answer: String, // answer_or_triples(PoCでは平文回答のみ)
    pub embedding: Vec<f32>, // 正規化済み
    pub created: String, // ISO8601(claimed_date 相当・平文)
    pub volatility: String, // permanent | slow | volatile
    pub shareable: bool,
    pub share_reason: String,
    pub agent: String,
    pub author_pub: String,
    pub author_sig: String,
    pub entry_id: String, // sha256(signed_payload) = content hash
}

impl CacheEntry {
    // 署名対象 = 質問 + 回答 + 日付 + 揮発性(キー順ソートで正規化)
    pub fn signed_payload(&self) -> String {
        // serde_json::Map は既定(preserve_order未使用)でキーをソートして保持する
        let j = json!({
            "question": self.question,
            "answer": self.answer,
            "created": self.created,
            "volatility": self.volatility,
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

    pub fn register_entry(
        &mut self,
        question: &str,
        answer: &str,
        volatility: &str,
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
            volatility: volatility.to_string(),
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
