// 案4トリプル分解 + 述語オントロジーのテスト(triples.rs)。
//
// 検証対象:
//   - decompose() の成功/失敗ケース(日本語 開発文/所有格/コピュラ、英語)
//   - 述語オントロジーのクラス判定(permanent/slow/volatile 各代表、日英エイリアス、
//     大文字小文字非依存、未知述語 → None)
//   - 生成文フォールバックが「分解失敗(success=false)」になること
//   - unknown_predicates の記録
//   - 同一入力に対する決定性(§10.1「主観がなく再現可能」)
//
// 分解ヒューリスティックは §7.3 / §10.1 の「分解成功/失敗」「述語型」を
// 判定できる粒度のPoC実装。ここではその粒度で観測可能な性質のみ検証する。

use crate::triples::{decompose, is_timely_object, predicate_class, VolatilityClass};

// ------------------------------------------------------------------
// 述語オントロジー(predicate_class)
// ------------------------------------------------------------------

#[test]
fn predicate_class_permanent_representatives()
{
    // permanent 代表述語。canonical と英語 alias の双方が permanent を返す。
    assert_eq!(predicate_class("開発者"), Some(VolatilityClass::Permanent));
    assert_eq!(predicate_class("首都"), Some(VolatilityClass::Permanent));
    assert_eq!(predicate_class("種別"), Some(VolatilityClass::Permanent));
    // 英語 alias(小文字収録)も同じクラスに解決される。
    assert_eq!(predicate_class("developer"), Some(VolatilityClass::Permanent));
    assert_eq!(predicate_class("developed by"), Some(VolatilityClass::Permanent));
    assert_eq!(predicate_class("is-a"), Some(VolatilityClass::Permanent));
}

#[test]
fn predicate_class_slow_representatives()
{
    // slow 代表述語(ゆっくり変わる)。
    assert_eq!(predicate_class("本社所在地"), Some(VolatilityClass::Slow));
    assert_eq!(predicate_class("人口"), Some(VolatilityClass::Slow));
    assert_eq!(predicate_class("headquarters"), Some(VolatilityClass::Slow));
    assert_eq!(predicate_class("population"), Some(VolatilityClass::Slow));
}

#[test]
fn predicate_class_volatile_representatives()
{
    // volatile 代表述語(時事)。
    assert_eq!(predicate_class("最新版"), Some(VolatilityClass::Volatile));
    assert_eq!(predicate_class("株価"), Some(VolatilityClass::Volatile));
    assert_eq!(predicate_class("price"), Some(VolatilityClass::Volatile));
    assert_eq!(predicate_class("weather"), Some(VolatilityClass::Volatile));
}

#[test]
fn predicate_class_is_case_insensitive_for_english_aliases()
{
    // 照合は入力を小文字化して行う(英語 alias は小文字収録)。
    // 大文字・混在表記でも同一クラスに解決されなければならない。
    assert_eq!(predicate_class("DEVELOPER"), Some(VolatilityClass::Permanent));
    assert_eq!(predicate_class("Population"), Some(VolatilityClass::Slow));
    assert_eq!(predicate_class("  Price  "), Some(VolatilityClass::Volatile));
}

#[test]
fn predicate_class_unknown_is_none()
{
    // オントロジー未収録の述語は None(呼び出し側が §10.1 ルール3で slow 扱いする)。
    assert_eq!(predicate_class("色"), None);
    assert_eq!(predicate_class("好きな食べ物"), None);
    assert_eq!(predicate_class("nonsense_predicate"), None);
}

// ------------------------------------------------------------------
// decompose() 成功ケース
// ------------------------------------------------------------------

#[test]
fn decompose_ja_development_extracts_multiple_triples()
{
    // 日本語 開発文: 「SはDが YYYY年に 開発したOです」
    //   → (S,開発者,D) / (S,開発年,YYYY年) / (S,種別,O) を抽出する。
    let d = decompose("Winnyは金子勇氏が2002年に開発したP2Pファイル共有ソフトウェアです。");
    assert!(d.success, "開発文の分解に失敗した");
    // 述語の集合として 開発者 / 開発年 / 種別 が含まれること。
    let preds: Vec<&str> = d.triples.iter().map(|t| t.p.as_str()).collect();
    assert!(preds.contains(&"開発者"), "開発者トリプルがない: {preds:?}");
    assert!(preds.contains(&"開発年"), "開発年トリプルがない: {preds:?}");
    assert!(preds.contains(&"種別"), "種別トリプルがない: {preds:?}");
    // 開発者トリプルの主語・目的語。
    let dev = d.triples.iter().find(|t| t.p == "開発者").unwrap();
    assert_eq!(dev.s, "Winny");
    assert_eq!(dev.o, "金子勇氏");
    // 述語は全て permanent 型なので unknown は空。
    assert!(d.unknown_predicates.is_empty(), "既知述語なのに unknown に記録された: {:?}", d.unknown_predicates);
}

#[test]
fn decompose_ja_possessive_permanent_predicate()
{
    // 所有格文: 「SのPはOです」 → (S,P,O)。首都は permanent 型述語。
    let d = decompose("日本の首都は東京です。");
    assert!(d.success);
    assert_eq!(d.triples.len(), 1);
    let t = &d.triples[0];
    assert_eq!((t.s.as_str(), t.p.as_str(), t.o.as_str()), ("日本", "首都", "東京"));
    assert_eq!(predicate_class(&t.p), Some(VolatilityClass::Permanent));
    assert!(d.unknown_predicates.is_empty());
}

#[test]
fn decompose_en_developed_by_uses_alias_predicate()
{
    // 英語 開発文: "S was developed by O" → (S,"developed by",O)。
    // "developed by" は 開発者(permanent)の英語 alias。
    let d = decompose("Rust was developed by Mozilla.");
    assert!(d.success, "英語開発文の分解に失敗した");
    assert_eq!(d.triples.len(), 1);
    let t = &d.triples[0];
    assert_eq!(t.s, "Rust");
    assert_eq!(t.p, "developed by");
    assert_eq!(t.o, "Mozilla");
    assert_eq!(predicate_class(&t.p), Some(VolatilityClass::Permanent));
    assert!(d.unknown_predicates.is_empty());
}

// ------------------------------------------------------------------
// decompose() 失敗ケース + unknown_predicates
// ------------------------------------------------------------------

#[test]
fn decompose_generated_fallback_fails()
{
    // MockAgent の汎用フォールバック(生成文)は分解できない = success:false。
    // §7.3「分解不能 → 共有除外」の信号になる。
    let d = decompose("(モック回答) 「宇宙の起源は何ですか」への回答をここでLLMが生成します。");
    assert!(!d.success, "生成文フォールバックが分解成功と判定された: {:?}", d.triples);
    assert!(d.triples.is_empty());
}

#[test]
fn decompose_non_declarative_fails()
{
    // コピュラ(です/でした/である)で終わらない叙述文は分解不能。
    let d = decompose("さまざまな要因が複雑に絡み合っています。");
    assert!(!d.success, "非宣言文が分解成功と判定された: {:?}", d.triples);
}

#[test]
fn decompose_records_unknown_predicate()
{
    // 所有格分解は成功するが、述語がオントロジー未収録 → unknown_predicates に記録。
    // (トリプル自体は返し、揮発性判定側で §10.1 ルール3の slow デフォルトが効く)
    let d = decompose("犬の色は茶色です。");
    assert!(d.success);
    assert_eq!(d.triples.len(), 1);
    assert_eq!(d.triples[0].p, "色");
    assert!(predicate_class("色").is_none(), "前提: 色は未収録述語のはず");
    assert!(
        d.unknown_predicates.contains(&"色".to_string()),
        "未知述語が unknown_predicates に記録されていない: {:?}",
        d.unknown_predicates
    );
}

#[test]
fn decompose_is_deterministic()
{
    // 決定的ヒューリスティック: 同一入力には常に同一の分解結果(§10.1)。
    let input = "Winnyは金子勇氏が2002年に開発したP2Pファイル共有ソフトウェアです。";
    let a = decompose(input);
    let b = decompose(input);
    assert_eq!(a.success, b.success);
    assert_eq!(a.triples, b.triples, "同一入力で分解結果が非決定的になった");
    assert_eq!(a.unknown_predicates, b.unknown_predicates);
}

// ------------------------------------------------------------------
// fully_decomposed(脅威レビュー Medium-2: 未解析文が残るか)
// ------------------------------------------------------------------

#[test]
fn decompose_all_sentences_sets_fully_decomposed_true()
{
    // 全文が分解できた複数文 → fully_decomposed=true(未解析文なし)。
    // success(1文以上)より厳しい「全文成功」の条件を確認する。
    let d = decompose("日本の首都は東京です。トヨタの本社所在地は愛知県です。");
    assert!(d.success, "前提: 少なくとも1文は分解できるはず");
    assert!(
        d.fully_decomposed,
        "全文分解できたのに fully_decomposed=false: triples={:?}",
        d.triples
    );
    // 2文とも既知述語(首都=permanent / 本社所在地=slow)。
    assert_eq!(d.triples.len(), 2, "2文とも1トリプルずつ抽出されるはず: {:?}", d.triples);
    assert!(d.unknown_predicates.is_empty());
}

#[test]
fn decompose_partial_sets_fully_decomposed_false()
{
    // 1文目は分解成功(首都)、2文目は非宣言文で分解不能。
    //   → success=true(1文以上成功)だが fully_decomposed=false(未解析文が残る)。
    // この「部分分解」が pipeline 側で共有除外の入力になる(揮発性クラスは変えない)。
    let d = decompose("日本の首都は東京です。多くの利用者がいる。");
    assert!(d.success, "1文目は分解できるはず");
    assert!(
        !d.fully_decomposed,
        "未解析文が残るのに fully_decomposed=true になった: triples={:?}",
        d.triples
    );
    // 抽出できたのは1文目の首都トリプルのみ。
    assert_eq!(d.triples.len(), 1, "分解できた文は1つのはず: {:?}", d.triples);
    assert_eq!(d.triples[0].p, "首都");
}

#[test]
fn decompose_full_failure_sets_fully_decomposed_false()
{
    // 全文が分解不能 → success=false かつ fully_decomposed=false。
    let d = decompose("さまざまな要因が複雑に絡み合っています。");
    assert!(!d.success, "非宣言文は分解失敗のはず");
    assert!(!d.fully_decomposed, "分解失敗なら fully_decomposed も false のはず");
}

// ------------------------------------------------------------------
// is_timely_object(脅威レビュー Medium-1: コピュラ目的語の形状ガード)
// ------------------------------------------------------------------

#[test]
fn is_timely_object_flags_numbers_currency_and_timely_terms()
{
    // 数値・通貨単位・割合・時点語・西暦は「ある時点の値」の可能性が高く、
    // 時事シグナルとみなす(finalize_volatility が種別/is-a の permanent 昇格を抑止する入力)。
    assert!(is_timely_object("1000万円"), "円を含む数値が時事シグナルにならなかった");
    assert!(is_timely_object("100ドル"), "ドルが時事シグナルにならなかった");
    assert!(is_timely_object("50%"), "割合(%)が時事シグナルにならなかった");
    assert!(is_timely_object("現在のトップ"), "時点語(現在)が時事シグナルにならなかった");
    assert!(is_timely_object("最新のもの"), "時点語(最新)が時事シグナルにならなかった");
    assert!(is_timely_object("2024年モデル"), "西暦(数値連続)が時事シグナルにならなかった");
    assert!(is_timely_object("3位"), "単独数値(順位)が時事シグナルにならなかった");
}

#[test]
fn is_timely_object_ignores_pure_definitions()
{
    // 純粋な定義語(数値・通貨・時点語を含まない)は時事シグナルではない。
    assert!(!is_timely_object("モデル"));
    assert!(!is_timely_object("企業"));
    assert!(!is_timely_object("プロトコル"));
    assert!(!is_timely_object("ファイル共有ソフトウェア"));
}

#[test]
fn is_timely_object_ignores_digits_sandwiched_between_letters()
{
    // 英字に「両側」を挟まれた数字(P2P / H2O)は固有名・化学式の埋め込み数字であり、
    // 時事シグナルとみなさない(誤って volatile へ降格させないためのガード)。
    assert!(!is_timely_object("P2P"), "P2P の埋め込み数字が誤って時事シグナル扱いされた");
    assert!(!is_timely_object("H2O"), "H2O の埋め込み数字が誤って時事シグナル扱いされた");
    assert!(
        !is_timely_object("P2Pファイル共有ソフトウェア"),
        "P2P を含む定義文が誤って時事シグナル扱いされた"
    );

    // 既知の限界(実装懸念として報告): ガードは「数字の両側が ASCII 英字」の場合だけ
    // 除外する。MP3 のように数字が末尾(右側が英字でない)の場合は挟まれておらず、
    // 現状は時事シグナル扱い(true)になる。triples.rs の contains_standalone_number の
    // コメントは MP3 を除外例として挙げているが、実挙動はこのとおり食い違う。
    // ここでは現挙動をピン留めする(修正はしない)。
    assert!(
        is_timely_object("MP3"),
        "MP3 の現挙動が変わった(末尾数字は現状 true。ガード見直し時はここも更新)"
    );
}

// ------------------------------------------------------------------
// 全角数字対応(脅威再レビュー Medium(全角数字汚染)回帰テスト):
// is_number_char が全角数字 U+FF10〜U+FF19 も数字と判定するようになり、
// 「レートは１５０です」型の全角時事値が数値シグナルをすり抜けて
// permanent へ洗浄される経路(volatile→permanent 誤分類。§10.1 で最も危険)を塞ぐ。
// ------------------------------------------------------------------

#[test]
fn is_timely_object_flags_fullwidth_numbers_and_currency()
{
    // ASCII 数値・通貨額と対になる全角ケース。全角でも同様に時事シグナルとして
    // 検出されなければならない(修正前は ASCII のみ検出=全角は取りこぼしていた)。
    // 単独の全角数値。
    assert!(is_timely_object("150"), "ASCII 単独数値が時事シグナルにならなかった");
    assert!(is_timely_object("１５０"), "全角単独数値が時事シグナルにならなかった");
    // 全角の桁区切り(万)を含む数値。
    assert!(is_timely_object("1000万"), "ASCII 数値(万)が時事シグナルにならなかった");
    assert!(is_timely_object("１０００万"), "全角数値(万)が時事シグナルにならなかった");
    // 全角通貨額(全角数字 + 通貨単位)。数値検出・通貨単位検出のいずれでも拾えるが、
    // ここでは「全角数字を含む時事値」が確実に拾われることをピン留めする。
    assert!(is_timely_object("1500円"), "ASCII 通貨額(円)が時事シグナルにならなかった");
    assert!(is_timely_object("１５００円"), "全角通貨額(円)が時事シグナルにならなかった");
    assert!(is_timely_object("１５０ドル"), "全角通貨額(ドル)が時事シグナルにならなかった");
    // 全角西暦(数値連続として包含される)。
    assert!(is_timely_object("２０２４年モデル"), "全角西暦が時事シグナルにならなかった");
}

#[test]
fn is_timely_object_ignores_fullwidth_digits_sandwiched_between_letters()
{
    // 負例: 全角数字であっても「両側が ASCII 英字」で挟まれた場合は
    // 固有名・化学式の埋め込み数字とみなし、時事シグナルにしない
    // (contains_standalone_number の除外条件は数字が全角でも同じく効く)。
    // 実際上は稀だが、全角対応で除外ガードが壊れていないことを確認する。
    assert!(!is_timely_object("P２P"), "全角数字を挟む P２P が誤って時事シグナル扱いされた");
    assert!(!is_timely_object("H２O"), "全角数字を挟む H２O が誤って時事シグナル扱いされた");
    // 対照: 純粋な定義語(数値を含まない)は当然シグナルにならない。
    assert!(!is_timely_object("プロトコル"));
}

#[test]
fn decompose_extracts_fullwidth_year()
{
    // find_year は private のため、それを経由する decompose(開発文パターン)から
    // 全角西暦「２００２年」が開発年トリプルとして抽出されることを確認する
    // (修正前は ASCII 西暦のみ抽出。全角西暦は年として拾えていなかった)。
    let d = decompose("Winnyは金子勇氏が２００２年に開発したP2Pファイル共有ソフトウェアです。");
    assert!(d.success, "全角西暦を含む開発文の分解に失敗した");
    let year = d.triples.iter().find(|t| t.p == "開発年");
    assert!(year.is_some(), "開発年トリプルが抽出されなかった: {:?}", d.triples);
    assert_eq!(year.unwrap().o, "２００２年", "全角西暦が開発年として抽出されなかった");
    // 述語は全て permanent 型(開発者/開発年/種別)なので unknown は空。
    assert!(d.unknown_predicates.is_empty(), "既知述語なのに unknown に記録された: {:?}", d.unknown_predicates);
}
