// 揮発性の確定ロジック finalize_volatility のテスト(volatility.rs / §10.1)。
//
// §10.1「初期ルール(安全側に倒す)」の4分岐を個別に検証する:
//   ルール2(最優先): 時間指示語 → 強制 volatile(質問と回答の両方を走査。
//                     脅威レビュー Medium-1 対応)
//   ルール1: 分解成功 かつ 全述語 permanent 型 → permanent
//   ルール3: 分解失敗 / 未知述語 → デフォルト slow
//   ルール4: 自己申告はクラスを動かさず、不一致なら確信度を下げるだけ
//
// 各テストの answer は「検証対象のルール以外を発火させない」中立な文を渡す
// (ルール2の回答側走査が入ったため、時事語を含む answer は volatile を強制する)。
//
// 誤分類コストの非対称性(§10.1)を明示的に検証する:
//   「疑わしきは slow/volatile 側へ」。permanent へ昇格するのは
//   「分解成功 かつ 全述語 permanent 型」の場合に限られ、
//   自己申告(信頼できないLLM)の Yes/permanent 側が class を permanent 側へ
//   動かすことは決してない(緩める経路の不在)。
//
// TripleDecomposition は pub フィールドを直接構築し、decompose() の
// ヒューリスティックから切り離して分岐を単独で検証する。

use crate::triples::{FactTriple, TripleDecomposition};
use crate::volatility::finalize_volatility;

// テスト用トリプル構築ヘルパー。
fn triple(s: &str, p: &str, o: &str) -> FactTriple
{
    FactTriple { s: s.to_string(), p: p.to_string(), o: o.to_string() }
}

fn decomp(triples: Vec<FactTriple>, success: bool, unknown: Vec<&str>) -> TripleDecomposition
{
    TripleDecomposition
    {
        triples,
        success,
        // finalize_volatility は fully_decomposed を参照しない(共有可否側の入力)。
        // ここでは success と同値にしておく。
        fully_decomposed: success,
        unknown_predicates: unknown.into_iter().map(|s| s.to_string()).collect(),
    }
}

// f32 の近似一致(確信度の定数比較用)。
fn approx(a: f32, b: f32) -> bool
{
    (a - b).abs() < 1e-4
}

// ------------------------------------------------------------------
// ルール1: 分解成功 かつ 全述語 permanent 型 → permanent
// ------------------------------------------------------------------

#[test]
fn rule1_all_permanent_predicates_promote_to_permanent()
{
    // 全述語が permanent 型かつ分解成功 → permanent に昇格(唯一の昇格経路)。
    // 自己申告も permanent にして不一致による確信度低下を排除し、初期値 0.6 を確認。
    let d = decomp(vec![triple("日本", "首都", "東京")], true, vec![]);
    let v = finalize_volatility("日本の首都はどこですか", "日本の首都は東京です。", &d, "permanent");
    assert_eq!(v.class, "permanent", "全 permanent 述語が permanent に昇格しなかった");
    assert!(approx(v.confidence, 0.6), "permanent 初期確信度が想定外: {}", v.confidence);
    assert!(v.evidence.iter().any(|e| e == "predicate_type:permanent"));
}

// ------------------------------------------------------------------
// ルール2(最優先): 時間指示語 → 強制 volatile
// ------------------------------------------------------------------

#[test]
fn rule2_time_term_forces_volatile_over_permanent_decomposition()
{
    // 分解結果は全て permanent 型だが、質問に時間指示語("最新")がある。
    // ルール2が最優先で class を volatile に強制する(分解結果に勝つ)。
    // これは「疑わしきは volatile 側」の非対称性の実証。
    let d = decomp(vec![triple("Claude", "種別", "モデル")], true, vec![]);
    let v = finalize_volatility("最新のClaudeモデルは何ですか", "Claudeはモデルです。", &d, "volatile");
    assert_eq!(v.class, "volatile", "時間指示語が最優先で volatile を強制しなかった");
    assert!(v.evidence.iter().any(|e| e.starts_with("time_term:")));
}

// ------------------------------------------------------------------
// ルール3: 分解失敗 / 未知述語 → デフォルト slow
// ------------------------------------------------------------------

#[test]
fn rule3_decomposition_failure_defaults_to_slow()
{
    // 分解失敗(生成文など) → デフォルト slow(permanent には決して落ちない)。
    let d = decomp(vec![], false, vec![]);
    let v = finalize_volatility("背景を説明してください", "さまざまな背景があります。", &d, "slow");
    assert_eq!(v.class, "slow", "分解失敗が slow デフォルトにならなかった");
    assert!(approx(v.confidence, 0.5), "分解失敗時の確信度が想定外: {}", v.confidence);
    assert!(v.evidence.iter().any(|e| e == "decomposition_failed"));
}

#[test]
fn rule3_unknown_predicate_defaults_to_slow_not_permanent()
{
    // 分解成功だが述語がオントロジー未収録 → slow(未知は permanent に昇格しない)。
    // 誤分類非対称性: 「知らない述語を permanent と誤るより slow で再検証する方が安全」。
    let d = decomp(vec![triple("犬", "色", "茶色")], true, vec!["色"]);
    let v = finalize_volatility("犬の色は何ですか", "犬の色は茶色です。", &d, "slow");
    assert_eq!(v.class, "slow", "未知述語が slow にならなかった(permanent 昇格は誤り)");
    assert!(approx(v.confidence, 0.5), "未知述語混在時の確信度が想定外: {}", v.confidence);
    assert!(v.evidence.iter().any(|e| e == "unknown_predicate:色"));
}

#[test]
fn mixed_predicates_take_most_volatile_class()
{
    // permanent と volatile 述語が混在 → 最も揮発側(volatile)を採用する。
    // (§10.1「最大揮発度を採用」= 安全側)
    let d = decomp(
        vec![triple("A社", "種別", "企業"), triple("A社", "株価", "3000円")],
        true,
        vec![],
    );
    // answer は時事語(株価/価格 等)を含まない文にして、ルール2(回答側走査)ではなく
    // 述語クラス由来の volatile 判定そのものを検証する。
    let v = finalize_volatility("A社の指標を教えてください", "A社の時価は3000円です。", &d, "volatile");
    assert_eq!(v.class, "volatile", "混在時に最大揮発度(volatile)を採用しなかった");
    assert!(v.evidence.iter().any(|e| e == "predicate_type:volatile"));
}

#[test]
fn known_slow_predicate_stays_slow_with_higher_confidence()
{
    // 既知 slow 述語のみ(未知なし) → slow、確信度は既知分の 0.6。
    let d = decomp(vec![triple("トヨタ", "本社所在地", "愛知県")], true, vec![]);
    let v = finalize_volatility("トヨタの本社所在地はどこですか", "トヨタの本社所在地は愛知県です。", &d, "slow");
    assert_eq!(v.class, "slow");
    assert!(approx(v.confidence, 0.6), "既知 slow 述語の確信度が想定外: {}", v.confidence);
    assert!(v.evidence.iter().any(|e| e == "predicate_type:slow"));
}

// ------------------------------------------------------------------
// ルール4: 自己申告はクラスを動かさない。不一致なら確信度を下げるだけ
// ------------------------------------------------------------------

#[test]
fn rule4_self_report_mismatch_lowers_confidence_but_not_class()
{
    // 分解は permanent を確定。自己申告は volatile(不一致)。
    // → class は permanent のまま不変、確信度のみ 0.6 * 0.7 = 0.42 に低下。
    // 自己申告が class を動かせないこと(決定でなく一票)の実証。
    let d = decomp(vec![triple("日本", "首都", "東京")], true, vec![]);
    let baseline = finalize_volatility("日本の首都はどこですか", "日本の首都は東京です。", &d, "permanent");
    let mismatched = finalize_volatility("日本の首都はどこですか", "日本の首都は東京です。", &d, "volatile");

    assert_eq!(mismatched.class, "permanent", "自己申告不一致で class が動いてしまった");
    assert_eq!(mismatched.class, baseline.class, "class は申告に依らず不変であるべき");
    assert!(
        mismatched.confidence < baseline.confidence,
        "不一致で確信度が下がっていない: mismatched={} baseline={}",
        mismatched.confidence,
        baseline.confidence
    );
    assert!(approx(mismatched.confidence, 0.42), "不一致確信度が 0.6*0.7 と異なる: {}", mismatched.confidence);
    assert!(mismatched.evidence.iter().any(|e| e.contains("self_report_mismatch")));
}

#[test]
fn rule4_self_report_yes_permanent_cannot_relax_forced_volatile()
{
    // 時間指示語で volatile を強制した上で、自己申告を permanent(=緩める方向)にしても
    // class は volatile のまま。Yes/permanent 側の申告が判定を緩める経路は存在しない。
    let d = decomp(vec![triple("Claude", "種別", "モデル")], true, vec![]);
    let v = finalize_volatility("最新のClaudeモデルは何ですか", "Claudeはモデルです。", &d, "permanent");
    assert_eq!(v.class, "volatile", "自己申告 permanent が強制 volatile を緩めてしまった");
    // 不一致(volatile vs permanent)なので確信度は低下方向のみ。
    assert!(v.confidence <= 0.9, "確信度が上振れした(緩める方向に作用した疑い): {}", v.confidence);
    assert!(v.evidence.iter().any(|e| e.contains("self_report_mismatch")));
}

// ------------------------------------------------------------------
// ルール2(回答側走査。脅威レビュー Medium-1): 質問が時間中立でも
// 回答に時事シグナルがあれば強制 volatile。
// ------------------------------------------------------------------

#[test]
fn rule2_answer_time_term_forces_volatile_when_question_is_neutral()
{
    // 質問は時間中立(時間指示語なし)だが、回答に時事語(現在/価格)がある。
    // 署名対象の answer を走査して強制 volatile に倒す(質問側だけ見ると
    // 取りこぼす時事シグナルを捕捉する)。分解結果は全 permanent 型だが勝てない。
    let d = decomp(vec![triple("A社", "種別", "企業")], true, vec![]);
    let v = finalize_volatility("A社について教えてください", "A社の価格は現在1000円です。", &d, "slow");
    assert_eq!(v.class, "volatile", "回答側の時事語が強制 volatile にならなかった");
    // 回答由来であることが evidence で区別できる(time_term_answer:)。
    assert!(
        v.evidence.iter().any(|e| e.starts_with("time_term_answer:")),
        "回答由来の time_term_answer evidence が無い: {:?}",
        v.evidence
    );
    // 質問由来(time_term:)ではないこと(質問は中立)。
    assert!(
        !v.evidence.iter().any(|e| e.starts_with("time_term:")),
        "質問は中立なのに質問由来 time_term evidence が付いた: {:?}",
        v.evidence
    );
}

// ------------------------------------------------------------------
// コピュラ目的語の形状ガード(脅威レビュー Medium-1):
// 「XはVです」型(種別 = permanent 型述語)でも目的語が時事シグナルを含むなら
// permanent 昇格せず volatile へ降格する(volatile→permanent 洗浄の防止)。
// ------------------------------------------------------------------

#[test]
fn copula_definition_with_timely_object_demoted_to_volatile()
{
    // 代表パターン: 円 / ドル / % / 時点語(現在) / 西暦。いずれも 種別 述語だが
    // 目的語が時事シグナルを含むため permanent 昇格を抑止し volatile へ降格する。
    // answer 側はルール2(回答側走査)を発火させない中立文にして、目的語形状ガード
    // そのものを分離して検証する(降格根拠 evidence が time_term_answer でないこと)。
    let objects = ["1000万円", "100ドル", "50%", "現在の首位", "2024年モデル"];
    for o in objects
    {
        let d = decomp(vec![triple("対象", "種別", o)], true, vec![]);
        let v = finalize_volatility("対象の定義を教えてください", "定義に関する説明です。", &d, "slow");
        assert_eq!(v.class, "volatile", "時事目的語({o})が volatile へ降格しなかった");
        // 降格根拠が目的語形状ガード由来であること。
        assert!(
            v.evidence.iter().any(|e| e == &format!("timely_definition_object:{o}")),
            "目的語形状ガードの evidence が無い(o={o}): {:?}",
            v.evidence
        );
        // permanent 昇格の evidence が付いていないこと(洗浄されていない)。
        assert!(
            !v.evidence.iter().any(|e| e == "predicate_type:permanent"),
            "時事目的語なのに permanent 昇格 evidence が付いた(o={o}): {:?}",
            v.evidence
        );
    }
}

#[test]
fn copula_definition_with_embedded_digit_stays_permanent()
{
    // 負例: 目的語が「英字に挟まれた数字」(P2P / H2O)や純粋な定義語のみで
    // 時事シグナルを含まない場合は、種別=permanent の昇格を維持する
    // (ガードが定義文を過剰に volatile 化しないことの確認)。
    let objects = ["P2Pファイル共有ソフトウェア", "H2O", "プロトコル"];
    for o in objects
    {
        let d = decomp(vec![triple("対象", "種別", o)], true, vec![]);
        let v = finalize_volatility("対象の定義を教えてください", "定義に関する説明です。", &d, "permanent");
        assert_eq!(v.class, "permanent", "定義目的語({o})が誤って降格した");
        assert!(
            v.evidence.iter().any(|e| e == "predicate_type:permanent"),
            "permanent 昇格 evidence が無い(o={o}): {:?}",
            v.evidence
        );
        // 目的語形状ガードは発火していないこと。
        assert!(
            !v.evidence.iter().any(|e| e.starts_with("timely_definition_object:")),
            "定義目的語なのに形状ガードが誤発火した(o={o}): {:?}",
            v.evidence
        );
    }
}

// ------------------------------------------------------------------
// 全角数字の目的語形状ガード(脅威再レビュー Medium(全角数字汚染)回帰テスト):
// 「レートは１５０です」「BTCは１０００万です」型のように、目的語が全角の
// 時事値(全角数値・全角通貨額)の場合も、種別=permanent の昇格を抑止して
// volatile へ降格する。修正前は is_timely_object が全角を取りこぼし、
// permanent へ洗浄されうる経路が残っていた(volatile→permanent が §10.1 で最も危険)。
// ------------------------------------------------------------------

#[test]
fn copula_definition_with_fullwidth_timely_object_demoted_to_volatile()
{
    // 全角の時事目的語。いずれも 種別 述語だが目的語が全角時事値のため
    // permanent 昇格を抑止し volatile へ降格する。answer 側はルール2(回答側走査)を
    // 発火させない中立文にして、目的語形状ガードそのものを分離して検証する。
    let objects = ["１５０", "１０００万", "１５００円", "１５０ドル", "２０２４年モデル"];
    for o in objects
    {
        let d = decomp(vec![triple("対象", "種別", o)], true, vec![]);
        let v = finalize_volatility("対象の定義を教えてください", "定義に関する説明です。", &d, "slow");
        assert_eq!(v.class, "volatile", "全角時事目的語({o})が volatile へ降格しなかった");
        // 降格根拠が目的語形状ガード由来であること(全角目的語がそのまま evidence に載る)。
        assert!(
            v.evidence.iter().any(|e| e == &format!("timely_definition_object:{o}")),
            "全角目的語の形状ガード evidence が無い(o={o}): {:?}",
            v.evidence
        );
        // permanent 昇格の evidence が付いていないこと(洗浄されていない)。
        assert!(
            !v.evidence.iter().any(|e| e == "predicate_type:permanent"),
            "全角時事目的語なのに permanent 昇格 evidence が付いた(o={o}): {:?}",
            v.evidence
        );
    }
}
