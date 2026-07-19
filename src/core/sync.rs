// ノードサービス(受信側検証・冪等マージ・announce処理・anti-entropy)。
// S3設計ノート §3「エントリ配送プロトコル」/ §4「一貫性と重複排除」/
// §6「モード分離」の実装本体。
//
// 構造(テスト前提の分離):
//   - NodeService はトランスポート非依存の「デーモンロジック」であり、
//     HTTP(daemon.rs)からも InMemoryTransport(transport.rs)からも
//     同じメソッドが呼ばれる(単一/マルチノードでコードパス共有)。
//   - テストは N 個の NodeService を1プロセスに起動し、InMemoryNetwork で
//     繋いで handle_announce / run_anti_entropy_once 等を直接呼べる。
//
// モード分離(§6): Private ノードは Delivery(transport+発見層)を
// インスタンス化しない(delivery: None)。announce / Transfer 供出 / Digest /
// anti-entropy は配送層の有無で構造的に不能になる。コンストラクタは
// Private + Some(delivery) の組を拒否する。

use crate::agent::{Agent, AgentError};
use crate::cache::{IngestOutcome, IngestReport, SemanticCache, LOCAL_THRESHOLD};
use crate::embedder::Embedder;
use crate::entry::{CacheEntry, Tier};
use crate::node::{node_id as node_id_of_pub, Mode, NodeIdentity};
use crate::pipeline::judge_entry;
use crate::policy::{DiscoveryPolicy, Policies};
use crate::transport::Transport;
use crate::triples::FactTriple;
use crate::wire::{digest_hash, Announce, Digest, DigestItem, Transfer};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

// ノード設定。TTL は §7「陳腐化」の検索除外(物理削除しない)に使う。
// 秒数は docs に確定値がない Phase1 暫定値であり、確定時は docs 側
// (Architecture §10 / S3設計ノート §7)と同時に更新すること。
pub struct NodeConfig
{
    pub mode: Mode,
    pub store_dir: PathBuf,        // §6: company_store/ と private_store/ を別ディレクトリに
    pub threshold: f32,            // 検索しきい値(既定 LOCAL_THRESHOLD=0.80)
    pub volatile_ttl_secs: i64,    // volatile のローカル短期TTL(暫定: 1時間)
    pub slow_ttl_secs: i64,        // slow の created+猶予(暫定: 30日)
}

impl NodeConfig
{
    pub fn new(mode: Mode, store_dir: PathBuf) -> Self
    {
        Self
        {
            mode,
            store_dir,
            threshold: LOCAL_THRESHOLD,
            volatile_ttl_secs: 60 * 60,
            slow_ttl_secs: 30 * 24 * 60 * 60,
        }
    }
}

// 配送層(Company のみ)。発見層ポリシー(§8-4点目の差し替え点)は
// 配送層に属する(Private は配送層ごと不在。§6)。
pub struct Delivery
{
    pub transport: Arc<dyn Transport>,
    pub discovery: Arc<dyn DiscoveryPolicy>,
}

// UI向け /v1/ask の結果(S3設計ノート §9 デーモンAPI)。
// answer: ヒット時は facts からの合成文(S2.5 は回答平文を保存しないため。
// poc の HIT 表示と同方針)。ミス時は Agent の生成回答そのもの。
#[derive(Debug, Clone, Serialize)]
pub struct AskResult
{
    pub hit: bool,
    pub answer: String,
    pub entry_id: String,
    pub similarity: f32,
    pub shareable: bool,
    pub tier: Tier,
    pub announced_to: usize, // announce を送達できたピア数(観測用。hit時は0)
}

// announce 受信処理の結果(§3: 通知→未知ならプル→検証→マージ)。
#[derive(Debug)]
pub enum AnnounceOutcome
{
    NoDelivery,           // 配送層なし(private 等)。構造上呼ばれない防御枝
    AlreadyKnown,         // 既知 entry_id(冪等)
    PeerUnknown(String),  // 通知元 node_id がピア表にない(プル先不明)
    PullFailed(String),   // Request/Transfer の転送失敗
    Rejected(String),     // 受信側検証(手順2〜9)で drop
    Ingested(IngestReport),
}

// anti-entropy 1周の観測(§10 テスト観点用に全て pub)。
#[derive(Debug, Default, Clone, Serialize)]
pub struct SyncReport
{
    pub peers_total: usize,     // 同期対象ピア数(自分を除く)
    pub peers_failed: usize,    // Digest 取得に失敗したピア数
    pub digests_matched: usize, // digest_hash 一致で全件比較を省略したピア数
    pub pulled: usize,          // 新規取り込み数
    pub already_known: usize,   // 既知でスキップした数
    pub pull_failed: usize,     // 実体プルの失敗数
    pub rejected: usize,        // 受信側検証で drop した数
}

// GET /v1/status の内容(§9)。
#[derive(Debug, Clone, Serialize)]
pub struct StatusReport
{
    pub node_id: String,
    pub mode: String,
    pub peers: usize,
    pub entries: usize,
    pub embedder: String,
    pub signer: String,
}

// GET /v1/entries/{entry_id} の内容(§9: facts/provenance/volatility)。
#[derive(Debug, Clone, Serialize)]
pub struct EntryDetail
{
    pub entry_id: String,
    pub question_key: String,
    pub question_norm: String,
    pub facts: Vec<FactTriple>,
    pub agent: String,
    pub model: String,
    pub embedder_model_id: String,
    pub created: String,
    pub initial_volatility_class: String,
    pub initial_tier: Tier,
    pub volatility_class_operative: String,
    pub volatility_confidence: f32,
    pub volatility_evidence: Vec<String>,
    pub shareable: bool,
    pub share_reason: String,
    pub tier_operative: Tier,
    pub author_pub: String,
    pub author_node_id: String, // author_pub から再計算(§1 追跡可能性)
}

pub struct NodeService
{
    config: NodeConfig,
    identity: NodeIdentity,
    embedder: Arc<dyn Embedder>,
    agent: Arc<dyn Agent>,
    policies: Policies,
    delivery: Option<Delivery>,
    cache: Mutex<SemanticCache>,
}

impl NodeService
{
    // 生成。モード分離の構造的強制(§6):
    //   - Private + 配送層あり → Err(送信経路を持たせない)
    //   - identity.mode と config.mode の不一致 → Err
    pub fn new(
        config: NodeConfig,
        identity: NodeIdentity,
        embedder: Arc<dyn Embedder>,
        agent: Arc<dyn Agent>,
        policies: Policies,
        delivery: Option<Delivery>,
    ) -> Result<Self, String>
    {
        if identity.mode != config.mode
        {
            return Err(format!(
                "identity({}) と config({}) の mode が不一致",
                identity.mode.as_str(),
                config.mode.as_str()
            ));
        }
        if config.mode == Mode::Private && delivery.is_some()
        {
            return Err(
                "--mode private では transport/sync/registry_client をインスタンス化しない(S3設計ノート §6)"
                    .to_string(),
            );
        }
        let cache = Mutex::new(SemanticCache::new(
            config.store_dir.clone(),
            embedder.clone(),
            identity.signer.clone(),
            config.threshold,
        ));
        Ok(Self
        {
            config,
            identity,
            embedder,
            agent,
            policies,
            delivery,
            cache,
        })
    }

    // ------------------------------------------------------------------
    // 参照系(テスト・daemon から使う)
    // ------------------------------------------------------------------

    pub fn mode(&self) -> Mode
    {
        self.config.mode
    }

    pub fn node_id(&self) -> &str
    {
        &self.identity.node_id
    }

    pub fn has_delivery(&self) -> bool
    {
        self.delivery.is_some()
    }

    pub fn entry_count(&self) -> usize
    {
        self.cache.lock().unwrap().size()
    }

    // キャッシュへの直接アクセス(テスト検査用。運用コードは専用メソッドを使う)。
    pub fn cache(&self) -> &Mutex<SemanticCache>
    {
        &self.cache
    }

    pub fn config(&self) -> &NodeConfig
    {
        &self.config
    }

    pub fn status(&self) -> StatusReport
    {
        let peers = self
            .delivery
            .as_ref()
            .map(|d| d.discovery.peers().iter().filter(|p| p.node_id != self.identity.node_id).count())
            .unwrap_or(0);
        StatusReport
        {
            node_id: self.identity.node_id.clone(),
            mode: self.config.mode.as_str().to_string(),
            peers,
            entries: self.entry_count(),
            embedder: self.embedder.name().to_string(),
            signer: self.identity.signer.name().to_string(),
        }
    }

    pub fn entry_detail(&self, entry_id: &str) -> Option<EntryDetail>
    {
        let cache = self.cache.lock().unwrap();
        let e = cache.get(entry_id)?;
        Some(EntryDetail
        {
            entry_id: e.entry_id.clone(),
            question_key: e.question_key.clone(),
            question_norm: e.core.question_norm.clone(),
            facts: e.core.facts.clone(),
            agent: e.core.provenance.agent.clone(),
            model: e.core.provenance.model.clone(),
            embedder_model_id: e.core.provenance.embedder_model_id.clone(),
            created: e.core.created.clone(),
            initial_volatility_class: e.core.initial_volatility_class.clone(),
            initial_tier: e.core.initial_tier,
            volatility_class_operative: e.state.volatility_class_operative.clone(),
            volatility_confidence: e.state.volatility_confidence,
            volatility_evidence: e.state.volatility_evidence.clone(),
            shareable: e.state.shareable,
            share_reason: e.state.share_reason.clone(),
            tier_operative: e.state.tier_operative,
            author_pub: e.author_pub.clone(),
            author_node_id: node_id_of_pub(&e.author_pub),
        })
    }

    // ------------------------------------------------------------------
    // UI経路: 質問(検索 → ミス時 Agent推論 → judge → 登録 → announce)
    // ------------------------------------------------------------------

    // 推論失敗(Err)時はエントリを一切登録しない: 失敗やゴミ回答を
    // キャッシュ・共有網に入れないことがヒット経路より優先(設計 2026-07-18 §4。
    // ヒット時は Agent を呼ばないため常に Ok)。
    pub fn ask(&self, question: &str) -> Result<AskResult, AgentError>
    {
        let now = Utc::now();
        // 検索(失効フィルタ+TTL除外フックを差し込む。§8-4/§7)
        let best_sim;
        {
            let cache = self.cache.lock().unwrap();
            let r = cache.lookup_filtered(question, &|e| self.is_searchable(e, now));
            if let Some(e) = r.entry
            {
                return Ok(AskResult
                {
                    hit: true,
                    answer: render_cached_answer(e),
                    entry_id: e.entry_id.clone(),
                    similarity: r.similarity,
                    shareable: e.state.shareable,
                    tier: e.state.tier_operative,
                    announced_to: 0,
                });
            }
            best_sim = r.similarity;
        } // ← ロック解放(Agent推論・announce をロック外で行う)

        // ミス: Agent 推論 + 判定パイプライン(Architecture §7 全段)。
        // 失敗はここで早期リターンし、以降の judge/登録/announce を行わない。
        let answer = self.agent.ask(question)?;
        let report = judge_entry(question, &answer, self.agent.as_ref());

        // 登録(ロック区間は登録のみに絞る。InMemoryTransport の announce が
        // 同一スレッドで相手ノード→自ノードへ折り返しても deadlock しないよう、
        // announce は必ずロック解放後に行う)
        let (entry_id, question_key, created, shareable, tier);
        {
            let mut cache = self.cache.lock().unwrap();
            let e = cache.register(
                question,
                &answer,
                &report.volatility,
                &report.decomposition.triples,
                report.shareable,
                &report.share_reason,
                self.agent.name(),
            );
            entry_id = e.entry_id.clone();
            question_key = e.question_key.clone();
            created = e.core.created.clone();
            shareable = e.state.shareable;
            tier = e.state.tier_operative;
        }

        // 配送announce(共有ゲート通過エントリのみ・best-effort。§3)
        let announced_to = if shareable
        {
            self.broadcast_announce(&entry_id, &question_key, &created)
        }
        else
        {
            0
        };

        Ok(AskResult
        {
            hit: false,
            answer,
            entry_id,
            similarity: best_sim,
            shareable,
            tier,
            announced_to,
        })
    }

    // 検索候補に含めるか(検索層のフック。cache::lookup_filtered へ渡す)。
    //   - CRL遡及除外(H-1・§7「既取込分は再検証で除外する」: 著者ノードが
    //     CRL失効したら、ingest 済みエントリも検索から即時除外する。
    //     再ロード不要。物理削除はしない)
    //   - 失効フィルタ(エントリ単位。§8-4差し替え点。Phase1 = 常にpass)
    //   - TTL検索除外(§7: volatile=短期 / slow=created+猶予 / permanent=無期限。
    //     物理削除はしない = grow-only 維持)
    fn is_searchable(&self, e: &CacheEntry, now: DateTime<Utc>) -> bool
    {
        if self.policies.cert.is_author_revoked(&e.author_pub)
        {
            return false; // 失効著者のエントリは既取込分も検索しない(H-1)
        }
        if self.policies.revocation.is_revoked(&e.entry_id)
        {
            return false;
        }
        let Ok(created) = DateTime::parse_from_rfc3339(&e.core.created)
        else
        {
            return false; // created が読めないエントリは保守側で除外
        };
        let age = (now - created.with_timezone(&Utc)).num_seconds();
        match e.state.volatility_class_operative.as_str()
        {
            "volatile" => age <= self.config.volatile_ttl_secs,
            "slow" => age <= self.config.slow_ttl_secs,
            _ => true, // permanent
        }
    }

    // ------------------------------------------------------------------
    // 配送: announce送信(§3「announceの配り方」= 既知ピア全員へ直接・best-effort)
    // ------------------------------------------------------------------

    fn broadcast_announce(&self, entry_id: &str, question_key: &str, created: &str) -> usize
    {
        let Some(d) = &self.delivery
        else
        {
            return 0; // private: 配送層が構造的に不在(§6)
        };
        let ann = Announce
        {
            entry_id: entry_id.to_string(),
            question_key: question_key.to_string(),
            created: created.to_string(),
            node_id: self.identity.node_id.clone(),
        };
        let mut sent = 0;
        for peer in d.discovery.peers()
        {
            if peer.node_id == self.identity.node_id
            {
                continue;
            }
            // best-effort: 失敗は無視(取りこぼしは anti-entropy が補償する)
            if d.transport.send_announce(&peer, &ann).is_ok()
            {
                sent += 1;
            }
        }
        sent
    }

    // ------------------------------------------------------------------
    // 配送: 受信側(wire ハンドラ。daemon.rs / InMemoryTransport から呼ばれる)
    // ------------------------------------------------------------------

    // POST /wire/announce 相当。未知エントリなら通知元からプルし、
    // §3手順1〜10 の受信側検証を経て冪等マージする。
    pub fn handle_announce(&self, ann: &Announce) -> AnnounceOutcome
    {
        let Some(d) = &self.delivery
        else
        {
            return AnnounceOutcome::NoDelivery;
        };
        // 既知なら何もしない(冪等)
        if self.cache.lock().unwrap().contains(&ann.entry_id)
        {
            return AnnounceOutcome::AlreadyKnown;
        }
        // プル先 = 通知元ノード(ピア表で node_id → URL を解決)
        let Some(peer) = d.discovery.peers().into_iter().find(|p| p.node_id == ann.node_id)
        else
        {
            return AnnounceOutcome::PeerUnknown(ann.node_id.clone());
        };
        let transfer = match d.transport.fetch_entry(&peer, &ann.entry_id)
        {
            Ok(t) => t,
            Err(e) => return AnnounceOutcome::PullFailed(e.to_string()),
        };
        match self.ingest_transfer(&transfer, Some(&ann.entry_id))
        {
            Ok(report) => AnnounceOutcome::Ingested(report),
            Err(reason) => AnnounceOutcome::Rejected(reason),
        }
    }

    // Transfer 受信の検証+取り込み(§3「受信側の検証手順」手順1〜10)。
    //   手順1   … エンベロープは transfer.envelope として既にパース済み
    //   手順2   … 組織PKI検証(CertPolicy=差し替え点§8-2。Phase2でwitness/評判へ)
    //   手順3〜9 … cache::verify_envelope(単一ノード load と共有のコードパス。
    //             author_sig 検証コアはここに固定 = ポリシーで差し替え不能)
    //   (時刻)… TimePolicy(差し替え点§8-3。Phase1はスキップ=組織時計信頼)
    //   手順10  … cache::insert_verified(冪等マージ・grow-only。§4)
    // 受信対象は .entry(core+署名)のみ。ピアの state.json はプロトコル上
    // 存在せず取り込まない(S2.5 §13 補足)。
    pub fn ingest_transfer(
        &self,
        transfer: &Transfer,
        expected_entry_id: Option<&str>,
    ) -> Result<IngestReport, String>
    {
        // 手順2: author_pub が有効な node_cert を持つか(CA署名OK + CRL未失効)
        self.policies
            .cert
            .verify_author(&transfer.envelope.author_pub)
            .map_err(|e| format!("node_cert検証失敗: {e}"))?;

        let mut cache = self.cache.lock().unwrap();

        // 手順3〜9: ハッシュ照合(改ざん検知)→署名検証(偽造防止)→parse→
        //           question_key/embedding再計算→運用値再導出(送信者値不信任)
        let entry = cache
            .verify_envelope(&transfer.envelope, expected_entry_id)
            .map_err(|e| e.to_string())?;

        // 時刻検証(差し替え点。Phase1 = OrgClockTimePolicy が常にpass)
        self.policies
            .time
            .verify_created(&entry.core.created)
            .map_err(|e| format!("時刻検証失敗: {e}"))?;

        // 手順10: 冪等マージ(同一entry_idスキップ・同一question_key異IDは併存)
        Ok(cache.insert_verified(entry))
    }

    // GET /wire/entry/{entry_id} 相当。供出条件は AND:
    //   - shareable=true(登録時 judge_entry / 取込時再導出の確定値。
    //     reload 後は M-2 の単調性保護により登録時判定を上回らない)
    //   - 著者が CRL 未失効(H-1・§7: 失効著者のエントリは既取込分も
    //     他ノードへ供出・再伝播しない)
    // shareable=false は「受け取るが自ノードでは共有伝播しない」
    // (§3手順9注記/§7 過失毒対処)。
    pub fn handle_entry_request(&self, entry_id: &str) -> Option<Transfer>
    {
        self.delivery.as_ref()?; // 配送層なし(private)は供出しない(§6)
        let cache = self.cache.lock().unwrap();
        let e = cache.get(entry_id)?;
        if !e.state.shareable
        {
            return None;
        }
        if self.policies.cert.is_author_revoked(&e.author_pub)
        {
            return None; // 失効著者のエントリは供出しない(H-1 遡及除外)
        }
        cache.envelope_for(entry_id).map(|envelope| Transfer { envelope })
    }

    // GET /wire/digest 相当。共有伝播対象(shareable=true かつ 著者CRL未失効)
    // のみ列挙する(H-1: 失効著者のエントリは Digest 経由の anti-entropy でも
    // 再伝播させない)。
    pub fn handle_digest_request(&self) -> Digest
    {
        if self.delivery.is_none()
        {
            // private: 配送層なし → 空のDigest(wireルート自体もマウントされない)
            return Digest { digest_hash: digest_hash(&[]), entries: Vec::new() };
        }
        let cache = self.cache.lock().unwrap();
        let mut items: Vec<DigestItem> = cache
            .entries()
            .iter()
            .filter(|e| e.state.shareable && !self.policies.cert.is_author_revoked(&e.author_pub))
            .map(|e| DigestItem
            {
                entry_id: e.entry_id.clone(),
                question_key: e.question_key.clone(),
            })
            .collect();
        items.sort_by(|a, b| a.entry_id.cmp(&b.entry_id));
        let ids: Vec<String> = items.iter().map(|i| i.entry_id.clone()).collect();
        Digest { digest_hash: digest_hash(&ids), entries: items }
    }

    // ------------------------------------------------------------------
    // anti-entropy(§3 Digest / §11-5: 定期ポーリング+Digestハッシュ比較)
    // ------------------------------------------------------------------

    // 全既知ピアと Digest を交換し、欠落分をプルして収束させる(1周分)。
    // 周期実行は呼び出し側(main.rs のポーリングスレッド / テスト)が担う。
    pub fn run_anti_entropy_once(&self) -> SyncReport
    {
        let mut rep = SyncReport::default();
        let Some(d) = &self.delivery
        else
        {
            return rep; // private: 同期経路なし(§6)
        };
        for peer in d.discovery.peers()
        {
            if peer.node_id == self.identity.node_id
            {
                continue;
            }
            rep.peers_total += 1;
            let remote = match d.transport.fetch_digest(&peer)
            {
                Ok(x) => x,
                Err(_) =>
                {
                    rep.peers_failed += 1;
                    continue;
                }
            };
            // ハッシュ一致なら全件比較を省略(直前のプル結果を反映するため毎回再計算)
            let local = self.handle_digest_request();
            if remote.digest_hash == local.digest_hash
            {
                rep.digests_matched += 1;
                continue;
            }
            for item in remote.entries
            {
                if self.cache.lock().unwrap().contains(&item.entry_id)
                {
                    rep.already_known += 1;
                    continue;
                }
                // 失効フィルタ(§8-4。Phase1 = 常にpass)
                if self.policies.revocation.is_revoked(&item.entry_id)
                {
                    continue;
                }
                match d.transport.fetch_entry(&peer, &item.entry_id)
                {
                    Ok(t) => match self.ingest_transfer(&t, Some(&item.entry_id))
                    {
                        Ok(r) => match r.outcome
                        {
                            IngestOutcome::Added => rep.pulled += 1,
                            IngestOutcome::Duplicate => rep.already_known += 1,
                        },
                        Err(_) => rep.rejected += 1,
                    },
                    Err(_) => rep.pull_failed += 1,
                }
            }
        }
        rep
    }
}

// ヒット時の提示文(S2.5 は回答平文を保存しない=facts からの合成。§1)。
// 受信側再合成の本格実装は将来課題であり、Phase1 は facts の直列表示とする。
fn render_cached_answer(e: &CacheEntry) -> String
{
    if e.core.facts.is_empty()
    {
        format!(
            "(キャッシュ命中: 「{}」。S2.5形式は回答平文を保存しないため、facts未収録エントリは要約を提示できません)",
            e.core.question_norm
        )
    }
    else
    {
        e.core
            .facts
            .iter()
            .map(|t| format!("{} {} {}。", t.s, t.p, t.o))
            .collect::<Vec<_>>()
            .join(" ")
    }
}
