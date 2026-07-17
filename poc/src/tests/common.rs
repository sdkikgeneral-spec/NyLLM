// テスト共通ヘルパー。
//
// 一時ディレクトリの生成のみを提供する。新規依存は追加せず、
// 既存の rand crate と std のみで実装する。

use rand::Rng;
use std::path::PathBuf;

// std::env::temp_dir() 配下に、他プロセス・他テストと衝突しない
// ユニークな一時ディレクトリを作成してそのパスを返す。
//
// 一意性の担保:
//   - プロセスID … 並行実行される別プロセスとの衝突を防ぐ
//   - rand乱数   … 同一プロセス内で複数回呼ばれた場合の衝突を防ぐ
//   - tag        … どのテストが作ったか判別できるようにする
pub(crate) fn temp_dir(tag: &str) -> PathBuf
{
    let pid = std::process::id();
    let nonce: u64 = rand::thread_rng().gen();
    let dir = std::env::temp_dir().join(format!("nyllm_poc_test_{}_{}_{:016x}", tag, pid, nonce));
    std::fs::create_dir_all(&dir).expect("テスト用一時ディレクトリの作成に失敗");
    dir
}
