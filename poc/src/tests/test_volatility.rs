// 揮発性分類と共有可否 AND ゲートのテスト(volatility.rs)。
//
// 共有ゲートは「文脈自立 AND 事実型(非主観・非個人) AND 非volatile」の AND。
// デフォルトは共有しない(保守的デフォルト)。ここでは各ブロック条件が
// 単独で不可判定を出すこと、および全通過時のみ可になることを個別に確認する。

use crate::volatility::{classify_volatility, share_gate};

#[test]
fn classify_time_referring_question_is_volatile()
{
    // 時間指示語("最新")を含む → volatile。
    let v = classify_volatility("最新のモデルは何ですか");
    assert_eq!(v, "volatile");
}

#[test]
fn classify_plain_question_is_slow()
{
    // 時間指示語を含まない事実質問 → slow(permanent 昇格は PoC では行わない)。
    let v = classify_volatility("水の沸点は摂氏何度ですか");
    assert_eq!(v, "slow");
}

#[test]
fn context_dependent_question_blocks_share()
{
    // 文脈依存語("それ")のみで不可。
    let d = share_gate("それについて詳しく説明してください", "slow");
    assert!(!d.shareable, "文脈依存語が共有ブロックしなかった: {}", d.reason);
}

#[test]
fn subjective_question_blocks_share()
{
    // 主観語("best")のみで不可(他の語彙を混入させない英文を使用)。
    let d = share_gate("what is the best editor", "slow");
    assert!(!d.shareable, "主観語が共有ブロックしなかった: {}", d.reason);
}

#[test]
fn personal_question_blocks_share()
{
    // 個人参照("私の")のみで不可。
    let d = share_gate("私の環境の設定を説明してください", "slow");
    assert!(!d.shareable, "個人参照が共有ブロックしなかった: {}", d.reason);
}

#[test]
fn volatile_alone_blocks_share()
{
    // 文脈・主観・個人語を含まない中立質問でも、volatility=="volatile" 単独で不可。
    // (質問文自体には VOLATILE_TERMS を混入させず、揮発性判定は引数で強制する)
    let d = share_gate("水の沸点は摂氏何度ですか", "volatile");
    assert!(!d.shareable, "volatile 単独で共有ブロックしなかった: {}", d.reason);
}

#[test]
fn all_clear_factual_slow_is_shareable()
{
    // AND ゲート全通過の唯一の可ケース:
    //   文脈依存・主観・個人語を一切含まず(事実型・文脈自立)、
    //   かつ volatility=="slow"(非volatile)。
    let q = "水の沸点は摂氏何度ですか";
    // 前提の再確認: この質問は slow に分類される。
    assert_eq!(classify_volatility(q), "slow");
    let d = share_gate(q, "slow");
    assert!(d.shareable, "全通過のはずが共有可にならなかった: {}", d.reason);
}
