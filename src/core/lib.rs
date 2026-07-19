// NyLLM コアライブラリ(src/core)— 分散セマンティックキャッシュのRustコア。
//
// poc/(S1/S2 参照実装・凍結)から S2.5 エントリ形式適用済みの実装を移植したもの
// (S3設計ノート §9「移植」。docs/S3_Company_Phase1_社内多ノード共有設計.md)。
// 移植にあたり、poc では cache.rs に統合されていた S2.5 エントリ型・正準化・
// entry_id/question_key 算出を entry.rs へ分離し、cache.rs は SemanticCache
// (load/verify・検索・登録)に専念させた(§9 のモジュール分割に対応)。
//
// 不変条件(CLAUDE.md / S2.5 §4-§6。黙って変えると設計が壊れる):
//   - entry_id = hex(sha256(core_bytes))。core_bytes は encode_core() が
//     serde 非依存の長さ接頭辞バイナリ(ドメインタグ nyllm/entry/v1\n)で生成する。
//   - question_key = hex(sha256("nyllm/qkey/v1\n" || lp_str(fold(question))))。
//   - ロード時検証: ハッシュ照合(改ざん検知)+ Signer::verify(偽造防止)。
//     失敗エントリは drop。運用値(shareable/tier/volatility)は必ず再導出する。
//   - 保守的共有ゲート(AND・既定共有不可)、LOCAL_THRESHOLD=0.80 / SHARED_THRESHOLD=0.90。
//
// S3 新規モジュール(設計ノート §9「新規(S3)」):
//   node            NodeId / 組織PKI(node_cert / CRL)/ Mode
//   policy          §8 ポリシー差し替え点4点(cert検証/時刻検証/失効フィルタ/発見層)。
//                   author_sig 検証コアは cache::verify_envelope に固定で差し替え不能
//   wire            nyllm-wire/v1(Announce/Request/Transfer/Digest)
//   transport       Transport trait + InMemoryTransport(テスト用・pub)
//                   + HttpTransport(feature "http")
//   sync            NodeService(受信検証・冪等マージ・announce処理・anti-entropy)
//   registry_client レジストリ join / peers / ca 取得(feature "http")
//   daemon          axum サーバ2系統(UI向け /v1/* + ノード間 /wire/*。feature "http")
// バイナリ nyllm-node(main.rs)が --mode company|private で配線する(§6)。
//
// S4 新規モジュール(docs/S4_Company_Phase1_層1内在信頼度先行設計.md):
//   trust           層1 内在信頼度の算出コア(純粋関数。版ペア間Jaccard平均=案A)。
//                   実運用パスは policy::TrustPolicy(5点目の差し替え点。既定=
//                   ランキング重み0)経由で呼ぶ。層1は助言のみ — 共有ゲート
//                   (shareable)には一切配線しない(S4 §4)。

pub mod agent;
pub mod cache;
pub mod embedder;
pub mod entry;
pub mod node;
pub mod pipeline;
pub mod policy;
pub mod signer;
pub mod sync;
pub mod transport;
pub mod triples;
pub mod trust;
pub mod volatility;
pub mod wire;

#[cfg(feature = "http")]
pub mod daemon;
#[cfg(feature = "http")]
pub mod registry_client;

// テストモジュールの配線(CLAUDE.md 規則4)。テスト本体は指示どおり
// src/tests/ に置き、クレートルート(lib.rs は src/core/ 配下)から
// #[path = "../tests/..."] で参照する。production ロジックは変更しない
// (poc/src/main.rs の #[cfg(test)] mod tests と同方針)。
#[cfg(test)]
mod tests
{
    // #[path] は nested mod のため src/core/tests/ 基準で解決される。
    // 実ファイルは src/tests/ にあるので ../../tests/ で辿る。
    #[path = "../../tests/common.rs"]
    mod common;
    // Agent層(選択可能な推論先。設計 2026-07-18)のテスト。
    #[path = "../../tests/test_agent.rs"]
    mod test_agent;
    #[path = "../../tests/test_node.rs"]
    mod test_node;
    #[path = "../../tests/test_wire.rs"]
    mod test_wire;
    #[path = "../../tests/test_cache_ingest.rs"]
    mod test_cache_ingest;
    #[path = "../../tests/test_sync.rs"]
    mod test_sync;
    // 脅威レビュー指摘 H-1 / M-2 / M-1 の回帰テスト。
    #[path = "../../tests/test_revocation.rs"]
    mod test_revocation;
    #[path = "../../tests/test_monotonic_shareable.rs"]
    mod test_monotonic_shareable;
    #[path = "../../tests/test_ca_pinning.rs"]
    mod test_ca_pinning;
    // ポリシー差し替えフック(TimePolicy/RevocationPolicy)の実効性テスト(§8-3/§8-4)。
    #[path = "../../tests/test_policy_hooks.rs"]
    mod test_policy_hooks;
    // S4 層1 内在信頼度(trust 算出コア/policy hook/再導出フック/ランキング配線。§8)。
    #[path = "../../tests/test_trust.rs"]
    mod test_trust;
    #[path = "../../tests/bench_lookup.rs"]
    mod bench_lookup;
    #[cfg(feature = "http")]
    #[path = "../../tests/test_daemon_http.rs"]
    mod test_daemon_http;
    // レジストリ(本物のハンドラ)+ registry_client + HttpTransport の実HTTP統合スモーク。
    #[cfg(feature = "http")]
    #[path = "../../tests/test_registry_integration.rs"]
    mod test_registry_integration;
}
