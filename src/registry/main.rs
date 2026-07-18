// nyllm-registry — 発見専用の軽量レジストリ(S3設計ノート §2 / §9)。
//
//   POST /registry/join  {node_id, url, node_cert} … 参加登録(node_id で upsert)
//   GET  /registry/peers                            … ピア一覧
//   GET  /registry/ca                               … CA公開鍵 + CRL の配布
//
// ハンドラ・ルータの実体は lib.rs(nyllm_registry::build_router)に置く
// (nyllm-core の統合テストが本物のハンドラを実HTTPで起動できるようにした抽出。
//  不変条件=「発見のみ・信頼判断を持たない」の解説も lib.rs 冒頭を参照)。
// 本ファイルはバイナリの配線(引数パース・CA束ファイルの読込・listener)のみ。
//
// 使い方:
//   nyllm-registry [--listen 127.0.0.1:7600] [--ca-file ca.json]
//     ca.json 例: {"ca_pub":"<hex>","crl":{"revoked":[]}}
//
// 状態はメモリ内のみ(Phase1 の最小実装。レジストリが落ちても既知ピア間の
// 同期は継続する設計 = §2 のため、永続化は必須ではない)。

use nyllm_registry::{build_router, default_ca_bundle};
use serde_json::Value;
use std::process::exit;

fn usage_exit(msg: &str) -> !
{
    eprintln!("エラー: {msg}");
    eprintln!("使い方: nyllm-registry [--listen ADDR] [--ca-file F]");
    exit(2);
}

#[tokio::main]
async fn main()
{
    let mut listen = "127.0.0.1:7600".to_string();
    let mut ca_file: Option<String> = None;
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len()
    {
        match argv[i].as_str()
        {
            "--listen" =>
            {
                i += 1;
                listen = argv.get(i).cloned().unwrap_or_else(|| usage_exit("--listen に値がありません"));
            }
            "--ca-file" =>
            {
                i += 1;
                ca_file = Some(argv.get(i).cloned().unwrap_or_else(|| usage_exit("--ca-file に値がありません")));
            }
            other => usage_exit(&format!("不明な引数: {other}")),
        }
        i += 1;
    }

    // CA束: ファイルがあればそのまま配布、なければ空の既定形
    let ca: Value = match &ca_file
    {
        Some(path) =>
        {
            let data = std::fs::read_to_string(path).unwrap_or_else(|e|
            {
                eprintln!("--ca-file の読込失敗: {e}");
                exit(1);
            });
            serde_json::from_str(&data).unwrap_or_else(|e|
            {
                eprintln!("--ca-file のパース失敗: {e}");
                exit(1);
            })
        }
        None => default_ca_bundle(),
    };

    let app = build_router(ca);

    let listener = match tokio::net::TcpListener::bind(&listen).await
    {
        Ok(l) => l,
        Err(e) =>
        {
            eprintln!("{listen} のバインドに失敗: {e}");
            exit(1);
        }
    };
    println!("[registry] 発見専用レジストリを {listen} で待受(信頼判断は持たない)");
    if let Err(e) = axum::serve(listener, app).await
    {
        eprintln!("サーバ実行エラー: {e}");
        exit(1);
    }
}
