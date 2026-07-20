// nyllm-node — ノードデーモンの起動・配線(S3設計ノート §6 / §9)。
//
// 使い方:
//   nyllm-node --mode company [--store-dir company_store] [--key keys/node.key]
//              [--listen 127.0.0.1:7700] [--url http://127.0.0.1:7700]
//              [--registry http://127.0.0.1:7600]
//              [--cert-file cert.json | --dev-ca-key keys/ca.key]
//              [--ca-pub <hex>] [--sync-interval-secs 30]
//   nyllm-node --mode private [--store-dir private_store] [--key keys/node.key]
//              [--listen 127.0.0.1:7701]
//
// モード分離(§6 起動時分離。推論時判定ではない):
//   - company: 配送層(HttpTransport+PeerTable)を配線し、レジストリへ join、
//     定期ポーリング(ピア更新+anti-entropy)スレッドを起動する。
//   - private: transport / sync(配送)/ registry_client を一切インスタンス化しない。
//     レジストリにも現れない。store_dir も既定で private_store/ に分離する。
//
// 証明書(§1): company には有効な node_cert が必要。
//   --cert-file  … 組織CAが発行した証明書(JSON)を読む(§11-2 推奨: 既存PKI流用)
//   --dev-ca-key … 開発用: ローカルのCA鍵で自己発行する(§11-2 代替: 軽量CA。
//                  CA秘密鍵がノードに置かれるため検証・デモ専用)
//   --ca-pub     … CA公開鍵(hex)の明示ピン留め(M-1 推奨)。指定するとレジストリ
//                  供給の CA公開鍵は無視される(信頼アンカーをレジストリに委ねない。
//                  §8-1)。未指定かつ --dev-ca-key も無い場合はレジストリ供給の
//                  初回値で TOFU 固定される(ブートストラップ限定。Phase1 既知制約 =
//                  初回供給時点のレジストリ compromise は検出できない。
//                  policy.rs / registry_client.rs のコメント参照)。

use nyllm_core::agent::{create_agent, AgentBackend, AgentConfig};
use nyllm_core::daemon;
use nyllm_core::embedder::create_embedder;
use nyllm_core::node::{issue_node_cert, load_identity, Mode, NodeCert};
use nyllm_core::policy::{CompanyCertPolicy, PeerTable, Policies, RejectAllCertPolicy};
use nyllm_core::registry_client::{refresh_once, RegistryClient};
use nyllm_core::signer::create_signer;
use nyllm_core::sync::{Delivery, NodeConfig, NodeService};
use nyllm_core::transport::HttpTransport;
use std::path::PathBuf;
use std::process::exit;
use std::sync::Arc;
use std::time::Duration;

struct Args
{
    mode: Mode,
    store_dir: PathBuf,
    key: PathBuf,
    listen: String,
    url: String,
    registry: Option<String>,
    cert_file: Option<PathBuf>,
    dev_ca_key: Option<PathBuf>,
    ca_pub: Option<String>, // M-1: CA公開鍵の明示ピン留め(hex)
    sync_interval_secs: u64,
    // 共有キルスイッチ(共有オフ+法的姿勢再定義スペック §3.1)。
    // 既定 true(on)。company 起動時のみ意味を持つ(private は元々送出経路が構造的に不在)。
    sharing_enabled: bool,
    // --sharing が明示指定されたか(private モードで無視する際の警告判定に使う)。
    sharing_specified: bool,
}

fn usage_exit(msg: &str) -> !
{
    eprintln!("エラー: {msg}");
    eprintln!(
        "使い方: nyllm-node --mode company|private [--store-dir D] [--key K] \
         [--listen ADDR] [--url URL] [--registry URL] [--cert-file F | --dev-ca-key F] \
         [--ca-pub HEX] [--sync-interval-secs N] [--sharing on|off]"
    );
    exit(2);
}

fn parse_args() -> Args
{
    let mut mode: Option<Mode> = None;
    let mut store_dir: Option<PathBuf> = None;
    let mut key: Option<PathBuf> = None;
    let mut listen = "127.0.0.1:7700".to_string();
    let mut url: Option<String> = None;
    let mut registry: Option<String> = None;
    let mut cert_file: Option<PathBuf> = None;
    let mut dev_ca_key: Option<PathBuf> = None;
    let mut ca_pub: Option<String> = None;
    let mut sync_interval_secs = 30u64;
    let mut sharing_enabled = true; // 既定 on(共有キルスイッチ§3.1)
    let mut sharing_specified = false;

    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len()
    {
        let take_value = |i: &mut usize| -> String
        {
            *i += 1;
            if *i >= argv.len()
            {
                usage_exit(&format!("{} に値がありません", argv[*i - 1]));
            }
            argv[*i].clone()
        };
        match argv[i].as_str()
        {
            "--mode" =>
            {
                let v = take_value(&mut i);
                mode = Some(Mode::parse(&v).unwrap_or_else(|| usage_exit(&format!("不明なmode: {v}"))));
            }
            "--store-dir" => store_dir = Some(PathBuf::from(take_value(&mut i))),
            "--key" => key = Some(PathBuf::from(take_value(&mut i))),
            "--listen" => listen = take_value(&mut i),
            "--url" => url = Some(take_value(&mut i)),
            "--registry" => registry = Some(take_value(&mut i)),
            "--cert-file" => cert_file = Some(PathBuf::from(take_value(&mut i))),
            "--dev-ca-key" => dev_ca_key = Some(PathBuf::from(take_value(&mut i))),
            "--ca-pub" => ca_pub = Some(take_value(&mut i)),
            "--sync-interval-secs" =>
            {
                sync_interval_secs = take_value(&mut i)
                    .parse()
                    .unwrap_or_else(|_| usage_exit("--sync-interval-secs は数値"));
            }
            "--sharing" =>
            {
                let v = take_value(&mut i);
                sharing_enabled = match v.as_str()
                {
                    "on" => true,
                    "off" => false,
                    other => usage_exit(&format!("--sharing は on|off: {other}")),
                };
                sharing_specified = true;
            }
            other => usage_exit(&format!("不明な引数: {other}")),
        }
        i += 1;
    }

    let mode = mode.unwrap_or_else(|| usage_exit("--mode company|private は必須"));
    // §6: company_store/ と private_store/ を別ディレクトリに(片方のみマウント)
    let store_dir = store_dir.unwrap_or_else(|| PathBuf::from(match mode
    {
        Mode::Company => "company_store",
        Mode::Private => "private_store",
    }));
    let key = key.unwrap_or_else(|| PathBuf::from("keys").join("node.key"));
    let url = url.unwrap_or_else(|| format!("http://{listen}"));
    Args
    {
        mode,
        store_dir,
        key,
        listen,
        url,
        registry,
        cert_file,
        dev_ca_key,
        ca_pub,
        sync_interval_secs,
        sharing_enabled,
        sharing_specified,
    }
}

// main は同期関数とする(重要): 起動配線は reqwest::blocking
// (RegistryClient::join / refresh_once)を使うため、tokio ランタイムの
// 非同期コンテキスト内では実行できない(blocking クライアントの内部ランタイムが
// async 文脈で drop されると panic する)。全ての配線を素の main スレッドで行い、
// 最後の daemon::serve だけを明示的に生成した Runtime で block_on する。
fn main()
{
    let args = parse_args();
    let embedder = Arc::from(create_embedder());
    // 推論先の選択(設計 2026-07-18 §5): NYLLM_AGENT_BACKEND=mock|ollama 等の
    // 環境変数で解決する(不正値は mock フォールバック + 警告)。
    let agent_config = AgentConfig::from_env();
    match agent_config.backend
    {
        AgentBackend::Ollama => println!(
            "[node] agent backend=ollama model={} endpoint={} timeout={}s",
            agent_config.model, agent_config.endpoint, agent_config.timeout_secs
        ),
        AgentBackend::Mock => println!("[node] agent backend=mock"),
    }
    let agent = Arc::from(create_agent(&agent_config));
    let identity = load_identity(&args.key, args.mode).unwrap_or_else(|e|
    {
        eprintln!("鍵のロードに失敗: {e}");
        exit(1);
    });
    println!(
        "[node] node_id={}... mode={} store={}",
        &identity.node_id[..16],
        args.mode.as_str(),
        args.store_dir.display()
    );

    let svc: Arc<NodeService> = match args.mode
    {
        // --------------------------------------------------------------
        // private: 配送層・レジストリを一切インスタンス化しない(§6)
        // --------------------------------------------------------------
        Mode::Private =>
        {
            // 共有キルスイッチ(§3.1): private では元々送出経路が構造的に不在
            // なので --sharing は無視する(指定時は警告ログのみ)。
            if args.sharing_specified
            {
                println!(
                    "[node] 警告: --sharing は private モードでは無視されます\
                     (配送層が構造的に不在のため既定で共有経路なし)"
                );
            }
            let policies = Policies::phase1(Arc::new(RejectAllCertPolicy));
            let config = NodeConfig::new(Mode::Private, args.store_dir.clone());
            Arc::new(
                NodeService::new(config, identity, embedder, agent, policies, None)
                    .unwrap_or_else(|e|
                    {
                        eprintln!("ノード初期化失敗: {e}");
                        exit(1);
                    }),
            )
        }
        // --------------------------------------------------------------
        // company: PKI + レジストリ発見 + HTTP配送
        // --------------------------------------------------------------
        Mode::Company =>
        {
            // 自ノードの node_cert を用意する
            let (own_cert, ca_verifier, dev_ca_pub): (NodeCert, Arc<dyn nyllm_core::signer::Signer>, Option<String>) =
                if let Some(cert_path) = &args.cert_file
                {
                    let data = std::fs::read_to_string(cert_path).unwrap_or_else(|e|
                    {
                        eprintln!("--cert-file の読込失敗: {e}");
                        exit(1);
                    });
                    let cert: NodeCert = serde_json::from_str(&data).unwrap_or_else(|e|
                    {
                        eprintln!("--cert-file のパース失敗: {e}");
                        exit(1);
                    });
                    // 検証用 Signer は自ノードの実装を使う(Ed25519 は任意インスタンスで
                    // 公開検証可能。DummySigner=MAC では組織PKIの公開検証は成立しないため
                    // 実運用は --features ed25519 ビルドが前提)
                    (cert, identity.signer.clone(), None)
                }
                else if let Some(ca_key) = &args.dev_ca_key
                {
                    // 開発用の軽量CA: ローカルCA鍵で自己発行(§11-2 代替)
                    let ca: Arc<dyn nyllm_core::signer::Signer> =
                        Arc::from(create_signer(ca_key).unwrap_or_else(|e|
                        {
                            eprintln!("--dev-ca-key のロード失敗: {e}");
                            exit(1);
                        }));
                    let expires = (chrono::Utc::now() + chrono::Duration::days(365))
                        .format("%Y-%m-%dT%H:%M:%SZ")
                        .to_string();
                    let cert = issue_node_cert(
                        ca.as_ref(),
                        identity.signer.public_key_hex(),
                        &expires,
                        &[Mode::Company],
                    );
                    let ca_pub = ca.public_key_hex().to_string();
                    (cert, ca, Some(ca_pub))
                }
                else
                {
                    usage_exit("--mode company には --cert-file または --dev-ca-key が必要");
                };

            // ポリシー(Phase1: CA検証 / 時刻スキップ / 失効pass / レジストリ発見)。
            // CA公開鍵のピン留め(M-1): 優先順は --ca-pub(明示ピン)>
            // --dev-ca-key 由来(ローカルCA=事実上のピン)> 空(未設定)。
            // 非空で構築すると CompanyCertPolicy がピン留めし、レジストリ供給の
            // CA公開鍵では上書きされない。空の場合のみレジストリ初回供給で
            // TOFU 固定される(refresh_once / policy.rs のコメント参照)。
            let pinned_ca_pub = args
                .ca_pub
                .clone()
                .or(dev_ca_pub)
                .unwrap_or_default();
            if pinned_ca_pub.is_empty()
            {
                println!(
                    "[node] 警告: CA公開鍵が未ピン(--ca-pub 未指定)。レジストリ供給の \
                     初回値で固定します(M-1 Phase1既知制約: 初回供給時点のレジストリ \
                     compromise は検出不能。--ca-pub の明示指定を推奨)"
                );
            }
            let cert_policy = Arc::new(CompanyCertPolicy::new(ca_verifier, &pinned_ca_pub));
            cert_policy.upsert_cert(own_cert.clone());
            let peer_table = Arc::new(PeerTable::new());
            let policies = Policies::phase1(cert_policy.clone());

            // 配送層(HTTP)
            let transport = Arc::new(HttpTransport::new().unwrap_or_else(|e|
            {
                eprintln!("HttpTransport 初期化失敗: {e}");
                exit(1);
            }));
            let delivery = Delivery { transport, discovery: peer_table.clone() };

            let config = NodeConfig::new(Mode::Company, args.store_dir.clone());
            let node_id = identity.node_id.clone();
            let svc = Arc::new(
                NodeService::new(config, identity, embedder, agent, policies, Some(delivery))
                    .unwrap_or_else(|e|
                    {
                        eprintln!("ノード初期化失敗: {e}");
                        exit(1);
                    }),
            );
            // 共有キルスイッチ(§3.1): --sharing off なら「共有オフで安全に立ち上げる」
            // (daemon::serve 前にトグルする。既定 on の場合は何もしない=非破壊)。
            if !args.sharing_enabled
            {
                svc.set_sharing_enabled(false);
            }

            // レジストリ参加 + 定期リフレッシュ(ピア/CA・CRL)+ anti-entropy
            if let Some(reg_url) = &args.registry
            {
                let reg = RegistryClient::new(reg_url).unwrap_or_else(|e|
                {
                    eprintln!("レジストリクライアント初期化失敗: {e}");
                    exit(1);
                });
                if let Err(e) = reg.join(&node_id, &args.url, &own_cert)
                {
                    // join 失敗は致命にしない(レジストリ一時停止でも既知ピア同期は
                    // 継続できる設計。§2)
                    println!("[node] レジストリjoin失敗(後で再試行): {e}");
                }
                if let Err(e) = refresh_once(&reg, &peer_table, &cert_policy)
                {
                    println!("[node] 初回ピア取得失敗: {e}");
                }
                let svc2 = svc.clone();
                let peer_table2 = peer_table.clone();
                let cert_policy2 = cert_policy.clone();
                let interval = Duration::from_secs(args.sync_interval_secs.max(1));
                let own_cert2 = own_cert.clone();
                let node_id2 = node_id.clone();
                let self_url = args.url.clone();
                // ポーリングスレッド(§11-5: 定期ポーリング+Digestハッシュ比較)。
                // reqwest::blocking を使うため tokio 外の OS スレッドで回す。
                std::thread::spawn(move ||
                {
                    loop
                    {
                        std::thread::sleep(interval);
                        // join は冪等(離脱→再参加・レジストリ再起動の自己修復)
                        if let Err(e) = reg.join(&node_id2, &self_url, &own_cert2)
                        {
                            println!("[node] レジストリjoin再試行失敗: {e}");
                        }
                        match refresh_once(&reg, &peer_table2, &cert_policy2)
                        {
                            Ok(_) =>
                            {
                                let rep = svc2.run_anti_entropy_once();
                                if rep.pulled > 0 || rep.rejected > 0
                                {
                                    println!(
                                        "[node] anti-entropy: pulled={} rejected={} known={} peers={}/{}",
                                        rep.pulled,
                                        rep.rejected,
                                        rep.already_known,
                                        rep.peers_total - rep.peers_failed,
                                        rep.peers_total
                                    );
                                }
                            }
                            Err(e) => println!("[node] ピア更新失敗: {e}"),
                        }
                    }
                });
            }
            else
            {
                println!("[node] --registry 未指定: ピア発見なしの単独companyノードとして稼働");
            }
            svc
        }
    };

    // 共有キルスイッチ(§3.1): 起動ログに現在の共有状態を明示する
    // (svc.is_sharing_enabled() は company/private いずれの実際の値も反映する)。
    println!("[node] sharing={}", if svc.is_sharing_enabled() { "on" } else { "off" });

    // HTTPサーバのみ tokio ランタイム上で実行する
    let rt = tokio::runtime::Runtime::new().unwrap_or_else(|e|
    {
        eprintln!("tokioランタイム初期化失敗: {e}");
        exit(1);
    });
    if let Err(e) = rt.block_on(daemon::serve(svc, &args.listen))
    {
        eprintln!("{e}");
        exit(1);
    }
}
