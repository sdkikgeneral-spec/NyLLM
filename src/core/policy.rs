// ポリシー差し替え点(S3設計ノート §8「焼き込むと作り直しになる箇所」の4点)。
//
// Phase1 実装を trait のデフォルト実装体として提供し、Phase2 で差し替え可能な
// フックとして固定する:
//   (1) cert検証   CertPolicy       Phase1=組織CA検証(CompanyCertPolicy)
//                                   Phase2=witness/評判検証へ差替
//   (2) 時刻検証   TimePolicy       Phase1=スキップ(組織NTP信頼。OrgClockTimePolicy)
//                                   Phase2=アンカー検証へ差替
//   (3) 失効フィルタ RevocationPolicy Phase1=常にpass(NoRevocationPolicy)
//                                   Phase2=エントリ単位revocationへ差替
//   (4) 発見層     DiscoveryPolicy  Phase1=レジストリ由来のピア表(PeerTable)
//                                   Phase2=DHTへ差替
//
// ★差し替え「不能」なもの(§8-2): author_sig 検証コア(ハッシュ照合 +
// Signer::verify。cache::verify_envelope 内)はポリシーの対象外であり、
// どのポリシー実装に差し替えても必ず実行される。CertPolicy が検証するのは
// node_cert(著者鍵の組織的正当性)だけで、署名そのものの検証は代替しない。
//
// ★レジストリとの関係(§0・§8-1 最重要): DiscoveryPolicy は「どこにピアが
// いるか」を返すだけで、返されたピアやその node_cert を信頼するかは常に
// CertPolicy(=各ノードの自律検証)が判断する。「レジストリに載っている=信頼」
// はどの実装にも焼き込まない。

use crate::node::{cert_allows_mode, node_id, verify_node_cert, Crl, Mode, NodeCert, PeerInfo};
use crate::signer::Signer;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

// ------------------------------------------------------------------
// (1) cert検証ポリシー
// ------------------------------------------------------------------

pub trait CertPolicy: Send + Sync
{
    fn name(&self) -> &str;
    // author_pub(hex)が「正規ノードの鍵」として受け入れ可能か。
    // S3設計ノート §3 手順2(組織PKI検証)に対応する。Ok(()) でも
    // author_sig 検証(手順5)は別途必ず行われる(差し替え不能コア)。
    fn verify_author(&self, author_pub_hex: &str) -> Result<(), String>;

    // 【H-1: CRL遡及除外】author_pub(hex)の著者が失効済み(CRL掲載)か。
    //
    // §7「既取込分は再検証で除外する」の実装点。ingest 時の verify_author
    // (手順2)は「新規受信の門」であり、既に company_store に入った
    // 失効著者エントリには効かない。そこで検索(sync::is_searchable)・
    // 供出(handle_entry_request)・Digest列挙(handle_digest_request)の
    // 各パスが本メソッドで都度照合し、失効著者のエントリを遡及的に
    // 検索・再伝播から外す(物理削除はしない = grow-only 維持)。
    //
    // verify_author(フル検証)でなく CRL照合のみに絞る理由:
    //   - private ノード(RejectAllCertPolicy)は外部受信を全拒否するが、
    //     自ノードのローカルキャッシュ検索は生きていなければならない。
    //     フル検証を検索パスに敷くと自エントリまで検索不能になる。
    //   - company ノードもレジストリ初回取得前(CA pub 未ブートストラップ)
    //     に自キャッシュの検索が全滅してはならない。
    //   失効(CRL)は「以後この鍵の成果物を使うな」という組織の明示宣言
    //   なので、これだけを遡及させるのが §7 の要求と過不足なく一致する。
    //
    // 既定実装は false(= CRL という概念を持たないポリシーでは失効なし)。
    fn is_author_revoked(&self, _author_pub_hex: &str) -> bool
    {
        false
    }
}

// Phase1 実装: 組織CA検証(§1)。
//   - certs: author_pub(hex)→ node_cert の表。レジストリのピア一覧・自ノード
//     登録から充填される(install_peer_certs / upsert_cert)。
//   - 判定: cert が存在 AND CA署名OK AND 期限内 AND CRL未失効 AND company許可。
pub struct CompanyCertPolicy
{
    ca_verifier: Arc<dyn Signer>,
    ca_pub: RwLock<String>,
    // 【M-1: 信頼アンカーのピン留め】構築時にローカル設定(--ca-pub /
    // --dev-ca-key 等)で CA公開鍵が与えられたか。true の間、set_ca_pub
    // (レジストリ供給)は無視される(レジストリに信頼判断を持たせない。§8-1)。
    ca_pub_pinned: bool,
    certs: RwLock<HashMap<String, NodeCert>>,
    crl: RwLock<Crl>,
}

impl CompanyCertPolicy
{
    // ca_pub_hex が非空 = ローカル設定済みとしてピン留めする(M-1)。
    // 空文字で構築した場合のみ、レジストリ由来の初回供給(set_ca_pub)で
    // ブートストラップできる(その後は TOFU 固定。set_ca_pub 参照)。
    pub fn new(ca_verifier: Arc<dyn Signer>, ca_pub_hex: &str) -> Self
    {
        Self
        {
            ca_verifier,
            ca_pub: RwLock::new(ca_pub_hex.to_string()),
            ca_pub_pinned: !ca_pub_hex.is_empty(),
            certs: RwLock::new(HashMap::new()),
            crl: RwLock::new(Crl::default()),
        }
    }

    // 【M-1】レジストリ /registry/ca 供給の CA公開鍵を反映する — ただし
    // 「ローカル未設定時のブートストラップ」に限る:
    //   - 構築時にピン留め済み(ca_pub_pinned)なら常に無視する。
    //   - 未ピンでも、一度値が入った後は上書きしない(TOFU: 初回供給で固定)。
    //     毎ポーリングでの無条件上書きを許すと、レジストリ compromise 一発で
    //     信頼アンカーごと差し替えられ、偽CAの cert 群が全ノードで有効になる。
    // 反映されたら true を返す(呼び出し側=registry_client がログする)。
    // CAローテーションは Phase1 ではノード再起動+明示設定で行う(レジストリ
    // 経由の自動ローテーションは信頼判断の委譲になるため実装しない。§8-1)。
    pub fn set_ca_pub(&self, ca_pub_hex: &str) -> bool
    {
        if self.ca_pub_pinned || ca_pub_hex.is_empty()
        {
            return false;
        }
        let mut cur = self.ca_pub.write().unwrap();
        if !cur.is_empty()
        {
            return false; // TOFU: ブートストラップ済み。以後の供給値は無視
        }
        *cur = ca_pub_hex.to_string();
        true
    }

    // CRL の更新(§7)。失効済みノードの鍵は以後の受信(verify_author=手順2)で
    // 拒否され、既取込分も is_author_revoked を照合する検索・供出・Digest の
    // 各パスから即時に遡及除外される(H-1。再ロードを待たない)。
    //
    // 【M-1: Phase1既知制約(コード側の記録。docs反映は design-docs 担当)】
    // Phase1 の CRL はレジストリが平文HTTPで配布する無署名データであり、
    // CA署名(CA_sign(CRL))の検証は未実装。したがってレジストリが
    // compromise された場合、CRL の改ざんが成立しうる:
    //   - 失効ノードの復活(revoked からの削除)= 侵害鍵の成果物が再流通する
    //   - 正規ノードの検閲(revoked への追加)= 正規エントリが検索・伝播から
    //     消える(可用性攻撃。偽造はできない=author_sig 検証コアは不変)
    // CA公開鍵ピン留め(上記)により信頼アンカー自体の差し替えは防ぐが、
    // CRL の完全性は Phase2 で「CA署名付きCRL + CA pub のアウトオブバンド
    // 固定配布」を実装するまで、レジストリの運用防御(アクセス制御)に依存する。
    pub fn set_crl(&self, crl: Crl)
    {
        *self.crl.write().unwrap() = crl;
    }

    pub fn upsert_cert(&self, cert: NodeCert)
    {
        self.certs.write().unwrap().insert(cert.node_pub.clone(), cert);
    }

    // ピア一覧(レジストリ由来)の node_cert をまとめて表へ入れる。
    // ここでは検証しない(表への格納のみ)。検証は verify_author が
    // 参照の都度行う(レジストリ経由のデータを無検証で信頼しない)。
    pub fn install_peer_certs(&self, peers: &[PeerInfo])
    {
        let mut certs = self.certs.write().unwrap();
        for p in peers
        {
            if let Some(c) = &p.node_cert
            {
                certs.insert(c.node_pub.clone(), c.clone());
            }
        }
    }
}

impl CertPolicy for CompanyCertPolicy
{
    fn name(&self) -> &str
    {
        "company-ca(Phase1)"
    }

    fn verify_author(&self, author_pub_hex: &str) -> Result<(), String>
    {
        let cert = self
            .certs
            .read()
            .unwrap()
            .get(author_pub_hex)
            .cloned()
            .ok_or_else(|| "author_pub に対応する node_cert が未登録".to_string())?;
        let ca_pub = self.ca_pub.read().unwrap().clone();
        if ca_pub.is_empty()
        {
            return Err("CA公開鍵が未設定".to_string());
        }
        // CA署名 + node_id整合 + 有効期限(node.rs)
        verify_node_cert(self.ca_verifier.as_ref(), &ca_pub, &cert, Utc::now())?;
        // CRL照合(失効チェック。§1・§7)
        if self.crl.read().unwrap().is_revoked(&cert.node_id)
        {
            return Err(format!("CRL失効済みノード({})", &cert.node_id[..16.min(cert.node_id.len())]));
        }
        // mode許可: company 配送に参加できる鍵か
        if !cert_allows_mode(&cert, Mode::Company)
        {
            return Err("node_cert に company モード許可がない".to_string());
        }
        Ok(())
    }

    // 【H-1】CRL遡及照合。node_id は author_pub から直接再計算する
    // (node_id = hex(sha256(tag || pub)) は決定的で、cert 表に該当 cert が
    //  残っているかに依存しない。verify_node_cert が cert.node_id と pub の
    //  整合を強制しているため、cert 経由の照合と常に同じ node_id になる)。
    fn is_author_revoked(&self, author_pub_hex: &str) -> bool
    {
        self.crl.read().unwrap().is_revoked(&node_id(author_pub_hex))
    }
}

// 全拒否ポリシー: Private モードのノードに配線する(受信取り込み経路は構造的に
// 不在だが、仮に呼ばれても全て拒否する多層防御。§6)。
pub struct RejectAllCertPolicy;

impl CertPolicy for RejectAllCertPolicy
{
    fn name(&self) -> &str
    {
        "reject-all(private)"
    }

    fn verify_author(&self, _author_pub_hex: &str) -> Result<(), String>
    {
        Err("このノードは外部エントリを受け入れない(private/未設定)".to_string())
    }
}

// 全許可ポリシー: テスト・Phase2差し替えの予行専用(§10「ポリシー差替(非破壊性)」
// の確認用)。cert検証をダミーに差し替えても author_sig 検証コアが不変であることを
// 検証するために存在する。運用では使用しない。
pub struct AllowAllCertPolicy;

impl CertPolicy for AllowAllCertPolicy
{
    fn name(&self) -> &str
    {
        "allow-all(テスト専用)"
    }

    fn verify_author(&self, _author_pub_hex: &str) -> Result<(), String>
    {
        Ok(())
    }
}

// ------------------------------------------------------------------
// (2) 時刻検証ポリシー
// ------------------------------------------------------------------

pub trait TimePolicy: Send + Sync
{
    fn name(&self) -> &str;
    // 受信エントリの created(UTC RFC3339)を受け入れるか。
    fn verify_created(&self, created: &str) -> Result<(), String>;
}

// Phase1 実装: 組織共通時計(NTP)信頼のため検証スキップ(§4・§8-3)。
// 「NTPだから正しい」を検証ロジックに埋めず、スキップという明示的ポリシーに
// しておく。Phase2 はここをアンカー検証実装に差し替える。
pub struct OrgClockTimePolicy;

impl TimePolicy for OrgClockTimePolicy
{
    fn name(&self) -> &str
    {
        "org-clock-skip(Phase1)"
    }

    fn verify_created(&self, _created: &str) -> Result<(), String>
    {
        Ok(())
    }
}

// ------------------------------------------------------------------
// (3) 失効フィルタポリシー(エントリ単位。検索層のフック)
// ------------------------------------------------------------------

pub trait RevocationPolicy: Send + Sync
{
    fn name(&self) -> &str;
    // entry_id が失効済みか(検索から除外すべきか)。
    fn is_revoked(&self, entry_id: &str) -> bool;
}

// Phase1 実装: 常にpass(§8-4)。grow-only 前提を検索に焼き込まないための
// フックであり、Phase2 の revocation はここを差し替えるだけで載る。
pub struct NoRevocationPolicy;

impl RevocationPolicy for NoRevocationPolicy
{
    fn name(&self) -> &str
    {
        "no-revocation(Phase1)"
    }

    fn is_revoked(&self, _entry_id: &str) -> bool
    {
        false
    }
}

// ------------------------------------------------------------------
// (4) 発見層ポリシー
// ------------------------------------------------------------------

pub trait DiscoveryPolicy: Send + Sync
{
    fn name(&self) -> &str;
    // 既知ピアの現在のスナップショット(自ノードを含みうる。呼び出し側が
    // node_id で自分を除外する)。
    fn peers(&self) -> Vec<PeerInfo>;
}

// Phase1 実装: ピア表(§2 中央レジストリ発見)。
//   - 運用: registry_client がレジストリをポーリングして set_peers で更新する。
//   - テスト: InMemory 構成で固定ピアを set_peers して使う。
// Phase2 はこの trait を DHT 発見の実装に差し替える(検証レイヤは不変)。
#[derive(Default)]
pub struct PeerTable
{
    peers: RwLock<Vec<PeerInfo>>,
}

impl PeerTable
{
    pub fn new() -> Self
    {
        Self::default()
    }

    pub fn set_peers(&self, peers: Vec<PeerInfo>)
    {
        *self.peers.write().unwrap() = peers;
    }
}

impl DiscoveryPolicy for PeerTable
{
    fn name(&self) -> &str
    {
        "registry-peer-table(Phase1)"
    }

    fn peers(&self) -> Vec<PeerInfo>
    {
        self.peers.read().unwrap().clone()
    }
}

// ------------------------------------------------------------------
// 束(NodeService への注入単位)
// ------------------------------------------------------------------

// 検証系ポリシーの束。発見層(DiscoveryPolicy)は配送層(sync::Delivery)側に
// 属する(Private モードは配送層ごと不在になるため。§6)。
pub struct Policies
{
    pub cert: Arc<dyn CertPolicy>,
    pub time: Arc<dyn TimePolicy>,
    pub revocation: Arc<dyn RevocationPolicy>,
}

impl Policies
{
    // Phase1 既定の組(cert のみ注入。時刻=スキップ、失効=常にpass)。
    pub fn phase1(cert: Arc<dyn CertPolicy>) -> Self
    {
        Self
        {
            cert,
            time: Arc::new(OrgClockTimePolicy),
            revocation: Arc::new(NoRevocationPolicy),
        }
    }
}
