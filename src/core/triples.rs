// 案4 知識グラフ分解(トリプル分解)+ 述語オントロジー(Architecture §7.3, §10.1)。
//
//  回答文を (s, p, o) の事実トリプルへ「決定的ヒューリスティック」で分解する。
//  述語オントロジーは各述語に揮発性クラス(permanent / slow / volatile)を
//  事前付与した静的表(Architecture §10.1 案4:「主観がなく再現可能=攻撃者が
//  動かしにくい」)。完全なNLPは範囲外であり、「分解成功/失敗」
//  「述語がpermanent型か/未知か」を判定できる粒度に留める。
//  多言語対応も範囲外(日本語+英語の代表述語のみ。Roadmap §3 未解決事項)。
//
//  分解に失敗した回答(生成文)は §7.3 により共有除外の信号になる。
//  ただし揮発性クラスとしてはデフォルト slow(§10.1 ルール3)であり、
//  「除外(共有しない)」と「クラス slow(ローカル保持)」は別の判定である。

use serde::{Deserialize, Serialize};

// 揮発性クラス。宣言順 = 揮発度の昇順(Ord導出で「より揮発側」をmax比較できる)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum VolatilityClass
{
    Permanent,
    Slow,
    Volatile,
}

impl VolatilityClass
{
    pub fn as_str(self) -> &'static str
    {
        match self
        {
            VolatilityClass::Permanent => "permanent",
            VolatilityClass::Slow => "slow",
            VolatilityClass::Volatile => "volatile",
        }
    }
}

// 事実トリプル(Architecture §6 facts)。ImmutableCore.facts に保存され、
// 著者の主張内容そのものなので署名対象に含まれる(entry.rs::encode_core)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FactTriple
{
    pub s: String,
    pub p: String,
    pub o: String,
}

// 分解結果。success=false は「生成文/分解不能」(§7.3: 共有除外の信号)。
// unknown_predicates はオントロジー未収録の述語一覧(§10.1 ルール3の入力)。
#[derive(Debug, Clone)]
pub struct TripleDecomposition
{
    pub triples: Vec<FactTriple>,
    pub success: bool,
    // 脅威レビュー Medium-2 対応: 全ての文が分解できたか(未解析文が残る=false)。
    // success(1文以上分解できた)より厳しい条件。部分的にしか分解できていない
    // 回答は未解析部分に何が書かれているか検証できないため、pipeline の
    // 共有可否判定で共有不可に倒す入力になる(揮発性クラス自体は変えない)。
    pub fully_decomposed: bool,
    pub unknown_predicates: Vec<String>,
}

// 述語オントロジーの1エントリ。canonical は代表表記、aliases は同義表記
// (英語aliasは小文字で収録し、照合時に入力を小文字化して比較する)。
pub struct PredicateEntry
{
    pub canonical: &'static str,
    pub aliases: &'static [&'static str],
    pub class: VolatilityClass,
}

// 述語オントロジー(Architecture §10.1 案4)。
// 揮発性クラスの事前付与: 「Xの開発者はY」= permanent、「Xの最新版は」= volatile。
// 日本語+英語の代表述語のみ(初期構築方法・多言語対応は Roadmap §3 の未解決事項)。
pub const PREDICATE_ONTOLOGY: &[PredicateEntry] = &[
    // --- permanent: 歴史的事実・定義・物理定数(時間で変わらない) ---
    PredicateEntry
    {
        canonical: "開発者",
        aliases: &["developer", "developed by", "created by", "作者", "author", "発明者", "inventor", "設計者"],
        class: VolatilityClass::Permanent,
    },
    PredicateEntry
    {
        canonical: "種別",
        aliases: &["is-a", "type", "kind", "定義"],
        class: VolatilityClass::Permanent,
    },
    PredicateEntry
    {
        canonical: "開発年",
        aliases: &["リリース年", "発売年", "公開年", "release year", "released"],
        class: VolatilityClass::Permanent,
    },
    PredicateEntry
    {
        canonical: "首都",
        aliases: &["capital"],
        class: VolatilityClass::Permanent,
    },
    PredicateEntry
    {
        canonical: "生年月日",
        aliases: &["誕生日", "生年", "born", "birth date"],
        class: VolatilityClass::Permanent,
    },
    PredicateEntry
    {
        canonical: "化学式",
        aliases: &["chemical formula", "formula"],
        class: VolatilityClass::Permanent,
    },
    PredicateEntry
    {
        canonical: "原子番号",
        aliases: &["atomic number"],
        class: VolatilityClass::Permanent,
    },
    PredicateEntry
    {
        canonical: "沸点",
        aliases: &["boiling point"],
        class: VolatilityClass::Permanent,
    },
    PredicateEntry
    {
        canonical: "語源",
        aliases: &["由来", "origin", "etymology"],
        class: VolatilityClass::Permanent,
    },
    // --- slow: ゆっくり変わる(TTL付きで再検証する) ---
    PredicateEntry
    {
        canonical: "本社所在地",
        aliases: &["本社", "headquarters", "hq"],
        class: VolatilityClass::Slow,
    },
    PredicateEntry
    {
        canonical: "代表者",
        aliases: &["ceo", "社長", "代表"],
        class: VolatilityClass::Slow,
    },
    PredicateEntry
    {
        canonical: "人口",
        aliases: &["population"],
        class: VolatilityClass::Slow,
    },
    PredicateEntry
    {
        canonical: "所属",
        aliases: &["affiliation", "member of"],
        class: VolatilityClass::Slow,
    },
    PredicateEntry
    {
        canonical: "従業員数",
        aliases: &["employees"],
        class: VolatilityClass::Slow,
    },
    // --- volatile: 時事(共有非対象。ローカル短期TTLのみ) ---
    PredicateEntry
    {
        canonical: "最新版",
        aliases: &["最新バージョン", "latest version", "current version"],
        class: VolatilityClass::Volatile,
    },
    PredicateEntry
    {
        canonical: "価格",
        aliases: &["値段", "price"],
        class: VolatilityClass::Volatile,
    },
    PredicateEntry
    {
        canonical: "株価",
        aliases: &["stock price"],
        class: VolatilityClass::Volatile,
    },
    PredicateEntry
    {
        canonical: "天気",
        aliases: &["weather"],
        class: VolatilityClass::Volatile,
    },
    PredicateEntry
    {
        canonical: "順位",
        aliases: &["ランキング", "ranking", "rank"],
        class: VolatilityClass::Volatile,
    },
    PredicateEntry
    {
        canonical: "為替レート",
        aliases: &["exchange rate"],
        class: VolatilityClass::Volatile,
    },
];

// 述語 → 揮発性クラスの照合。未収録なら None(呼び出し側が §10.1 ルール3で
// slow 扱いにする。「未知は安全側」の非対称原則)。
pub fn predicate_class(predicate: &str) -> Option<VolatilityClass>
{
    let norm = predicate.trim().to_lowercase();
    PREDICATE_ONTOLOGY
        .iter()
        .find(|e| e.canonical == norm || e.aliases.iter().any(|a| *a == norm))
        .map(|e| e.class)
}

// ------------------------------------------------------------------
// 分解ヒューリスティック本体
// ------------------------------------------------------------------

// 疑問語(回答から抽出したはずの o/s に混ざっていたら分解失敗として棄却する)
const INTERROGATIVES: &[&str] = &["何", "誰", "どこ", "どれ", "いつ", "なぜ", "どう"];

// 主語・目的語フラグメントの健全性チェック。
// 引用符・疑問符・疑問語を含む断片は「生成文の切れ端」とみなして棄却する
// (§7.3「生成文/分解不能 → 除外」を偽トリプル化させないためのガード)。
fn clean_fragment(fragment: &str, max_chars: usize) -> Option<&str>
{
    let f = fragment.trim();
    if f.is_empty() || f.chars().count() > max_chars
    {
        return None;
    }
    if f.chars().any(|c| "「」『』??\"".contains(c))
    {
        return None;
    }
    if INTERROGATIVES.iter().any(|w| f.contains(w))
    {
        return None;
    }
    Some(f)
}

// 語尾のコピュラ(です/でした/である)を必須として除去する。
// コピュラで終わらない断片は宣言文でないとみなし None(疑問文などの誤抽出防止)。
fn strip_copula(segment: &str) -> Option<&str>
{
    let s = segment.trim().trim_end_matches('。').trim();
    for cop in ["でした", "である", "です"]
    {
        if let Some(body) = s.strip_suffix(cop)
        {
            let body = body.trim();
            if !body.is_empty()
            {
                return Some(body);
            }
        }
    }
    None
}

// 数字1文字の判定: ASCII数字('0'-'9')または全角数字('０'-'９', U+FF10〜U+FF19)。
// 脅威再レビュー Medium(全角数字)対応: 従来は ASCII 数字しか見ておらず、
// 「レートは１５０です」型の全角時事値が数値シグナルをすり抜けて
// permanent 昇格+共有可へ洗浄されうる経路が残っていた(§10.1 の
// 誤分類コスト非対称性: volatile→permanent 誤りが最も危険)。
// find_year / contains_standalone_number の双方がこの判定を共有する。
fn is_number_char(c: char) -> bool
{
    c.is_ascii_digit() || ('\u{FF10}'..='\u{FF19}').contains(&c)
}

// 「NNNN年」(4桁西暦。ASCII/全角数字とも可)を探す。前後に数字が続く場合は年とみなさない。
fn find_year(text: &str) -> Option<String>
{
    let chars: Vec<char> = text.chars().collect();
    for i in 0..chars.len()
    {
        if i + 4 < chars.len()
            && chars[i..i + 4].iter().all(|c| is_number_char(*c))
            && chars[i + 4] == '年'
            && (i == 0 || !is_number_char(chars[i - 1]))
        {
            return Some(chars[i..=i + 4].iter().collect());
        }
    }
    None
}

// ------------------------------------------------------------------
// コピュラ目的語の形状ガード(脅威レビュー Medium-1 対応)
// ------------------------------------------------------------------

// 述語がコピュラ定義述語(種別/is-a 系)かどうか。
// オントロジーの「種別」エントリ(canonical + aliases)と照合する。
pub fn is_definition_predicate(predicate: &str) -> bool
{
    let norm = predicate.trim().to_lowercase();
    PREDICATE_ONTOLOGY
        .iter()
        .find(|e| e.canonical == "種別")
        .map(|e| e.canonical == norm || e.aliases.iter().any(|a| *a == norm))
        .unwrap_or(false)
}

// 「単独の数値」を含むか。ASCII/全角数字の連続を数値とみなすが
// (脅威再レビュー Medium(全角数字)対応: is_number_char が全角 U+FF10〜U+FF19 も
// 数字と判定する)、数字の並びの両側が ASCII 英字である場合(P2P / H2O のような
// 固有名・化学式の埋め込み数字)は数値シグナルとみなさない。
// 「1000万円」「１０００万」「Claude Opus 4」「2002年」(find_year が拾う西暦も
// ここに包含)は数値シグナルになる。MP3 のように数字が末尾の場合は「両側が英字」
// でないため現状は数値シグナル扱い(既知の安全側挙動。test_triples.rs でピン留め)。
fn contains_standalone_number(text: &str) -> bool
{
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    while i < chars.len()
    {
        if is_number_char(chars[i])
        {
            let start = i;
            while i < chars.len() && is_number_char(chars[i])
            {
                i += 1;
            }
            let left_letter = start > 0 && chars[start - 1].is_ascii_alphabetic();
            let right_letter = i < chars.len() && chars[i].is_ascii_alphabetic();
            if !(left_letter && right_letter)
            {
                return true;
            }
        }
        else
        {
            i += 1;
        }
    }
    false
}

// コピュラ目的語の「時事シグナル」判定(脅威レビュー Medium-1 対応)。
// 「SはOです」型で O が数値・通貨・年・時点語を含む場合、それは定義(permanent)
// ではなく「ある時点の値」である可能性が高い(例: 「ビットコインは1000万円です」)。
// 誤分類コストの非対称性(Architecture §10.1: volatile→permanent 誤りが最も危険)に
// 従い、疑わしきは volatile 側へ倒すための入力になる(volatility.rs::finalize_volatility
// が 種別/is-a の permanent 昇格をこの判定で抑止する)。
pub fn is_timely_object(o: &str) -> bool
{
    // 数値・年(西暦を含む。find_year 相当の「NNNN年」も数字連続として包含される)
    if contains_standalone_number(o)
    {
        return true;
    }
    // 通貨・割合の単位
    const CURRENCY_UNIT_TERMS: &[&str] = &["円", "¥", "$", "ドル", "ユーロ", "€", "%", "%"];
    if CURRENCY_UNIT_TERMS.iter().any(|t| o.contains(t))
    {
        return true;
    }
    // 時点語(時事シグナル)
    const TIMELY_TERMS: &[&str] =
        &["現在", "最新", "時点", "今", "今日", "current", "latest", "now", "today"];
    let lower = o.to_lowercase();
    TIMELY_TERMS.iter().any(|t| lower.contains(t))
}

// パターン1(日本語・開発文): 「SはDが(YYYY年に)開発したOです」
//   → (S, 開発者, D) / (S, 開発年, YYYY年) / (S, 種別, O)
fn decompose_ja_development(sentence: &str) -> Option<Vec<FactTriple>>
{
    let ha = sentence.find("は")?;
    let subj = clean_fragment(&sentence[..ha], 30)?;
    let rest = &sentence[ha + "は".len()..];
    let dev_idx = rest.find("開発した")?;
    let dev_part = &rest[..dev_idx];
    let ga_idx = dev_part.find("が")?;

    let mut triples = Vec::new();
    if let Some(developer) = clean_fragment(&dev_part[..ga_idx], 30)
    {
        triples.push(FactTriple
        {
            s: subj.to_string(),
            p: "開発者".to_string(),
            o: developer.to_string(),
        });
    }
    if let Some(year) = find_year(dev_part)
    {
        triples.push(FactTriple
        {
            s: subj.to_string(),
            p: "開発年".to_string(),
            o: year,
        });
    }
    let obj_part = &rest[dev_idx + "開発した".len()..];
    if let Some(body) = strip_copula(obj_part)
    {
        if let Some(o) = clean_fragment(body, 50)
        {
            triples.push(FactTriple
            {
                s: subj.to_string(),
                p: "種別".to_string(),
                o: o.to_string(),
            });
        }
    }
    if triples.is_empty()
    {
        None
    }
    else
    {
        Some(triples)
    }
}

// パターン2(日本語・所有格): 「SのPはOです」 → (S, P, O)
//   P は「の」と「は」に挟まれた短い名詞(≤6文字)のみ許す。
//   P がオントロジー未収録でもトリプルとしては返す(揮発性判定側で
//   §10.1 ルール3の slow デフォルトが効く)。
fn decompose_ja_possessive(sentence: &str) -> Option<Vec<FactTriple>>
{
    let mut search_from = 0usize;
    while let Some(no_rel) = sentence[search_from..].find("の")
    {
        let no_idx = search_from + no_rel;
        let after_no = &sentence[no_idx + "の".len()..];
        if let Some(ha_rel) = after_no.find("は")
        {
            let pred = &after_no[..ha_rel];
            let pred_ok = !pred.is_empty()
                && pred.chars().count() <= 6
                && pred.chars().all(|c| !"、。,. のは".contains(c));
            if pred_ok
            {
                if let Some(subj) = clean_fragment(&sentence[..no_idx], 30)
                {
                    let obj_seg = &after_no[ha_rel + "は".len()..];
                    if let Some(body) = strip_copula(obj_seg)
                    {
                        if let Some(o) = clean_fragment(body, 50)
                        {
                            return Some(vec![FactTriple
                            {
                                s: subj.to_string(),
                                p: pred.to_string(),
                                o: o.to_string(),
                            }]);
                        }
                    }
                }
            }
        }
        search_from = no_idx + "の".len();
    }
    None
}

// パターン3(日本語・コピュラ定義文): 「SはOです」 → (S, 種別, O)
//
// 脅威レビュー Medium-1 注記: このパターンは目的語 O の中身を見ずに
// permanent 型述語「種別」を割り当てるため、O が時事の値(「1000万円」等)でも
// permanent へ洗浄されうる。トリプル(facts。署名対象)は決定性維持のため
// このまま生成し、permanent 昇格の可否は finalize_volatility 側が
// is_timely_object(目的語形状ガード)で抑止する。
fn decompose_ja_copula(sentence: &str) -> Option<Vec<FactTriple>>
{
    let ha = sentence.find("は")?;
    let subj = clean_fragment(&sentence[..ha], 30)?;
    let obj_seg = sentence[ha + "は".len()..].trim_start_matches('、');
    let body = strip_copula(obj_seg)?;
    let o = clean_fragment(body, 50)?;
    Some(vec![FactTriple
    {
        s: subj.to_string(),
        p: "種別".to_string(),
        o: o.to_string(),
    }])
}

// パターン4(英語): "S was developed by O" / "The P of S is O" / "S is O"
//   ASCII文のみ対象(小文字化しても byte offset が揺れないようにするガード)。
//   "S is O"(is-a)も日本語コピュラ同様、目的語の時事シグナルによる
//   permanent 昇格の抑止は finalize_volatility 側で行う(脅威レビュー Medium-1)。
fn decompose_en(sentence: &str) -> Option<Vec<FactTriple>>
{
    if !sentence.is_ascii()
    {
        return None;
    }
    let lower = sentence.to_lowercase();

    const DEV_MARK: &str = " was developed by ";
    if let Some(i) = lower.find(DEV_MARK)
    {
        let subj = clean_fragment(&sentence[..i], 30);
        let obj = clean_fragment(sentence[i + DEV_MARK.len()..].trim_end_matches('.'), 50);
        if let (Some(s), Some(o)) = (subj, obj)
        {
            return Some(vec![FactTriple
            {
                s: s.to_string(),
                p: "developed by".to_string(),
                o: o.to_string(),
            }]);
        }
    }

    if lower.starts_with("the ")
    {
        if let Some(of_i) = lower.find(" of ")
        {
            if let Some(is_i) = lower[of_i..].find(" is ").map(|k| k + of_i)
            {
                let pred = sentence[4..of_i].trim();
                let subj = clean_fragment(&sentence[of_i + 4..is_i], 30);
                let obj = clean_fragment(sentence[is_i + 4..].trim_end_matches('.'), 50);
                if !pred.is_empty() && pred.split_whitespace().count() <= 3
                {
                    if let (Some(s), Some(o)) = (subj, obj)
                    {
                        return Some(vec![FactTriple
                        {
                            s: s.to_string(),
                            p: pred.to_lowercase(),
                            o: o.to_string(),
                        }]);
                    }
                }
            }
        }
    }

    if let Some(i) = lower.find(" is ")
    {
        let subj = clean_fragment(&sentence[..i], 30);
        let obj = clean_fragment(sentence[i + 4..].trim_end_matches('.'), 50);
        if let (Some(s), Some(o)) = (subj, obj)
        {
            // 長い主語節はコピュラ定義文とみなさない(生成文の誤抽出防止)
            if s.split_whitespace().count() <= 5
            {
                return Some(vec![FactTriple
                {
                    s: s.to_string(),
                    p: "is-a".to_string(),
                    o: o.to_string(),
                }]);
            }
        }
    }
    None
}

// 文分割(。 . ! ? ! ? で区切る)。疑問符でも切ることで、引用された
// 疑問文が後続の平叙文と混ざって誤抽出されるのを防ぐ。
fn split_sentences(text: &str) -> impl Iterator<Item = &str>
{
    text.split(|c: char| "。.!?!?".contains(c))
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

// 回答テキストを事実トリプルへ分解する(案4の実装)。
// 決定的ヒューリスティックであり、同一入力には常に同一の分解結果を返す
// (§10.1「主観がなく再現可能」の性質を保つ)。
pub fn decompose(answer: &str) -> TripleDecomposition
{
    let mut triples: Vec<FactTriple> = Vec::new();
    let mut total_sentences = 0usize;
    let mut decomposed_sentences = 0usize;
    for sentence in split_sentences(answer)
    {
        total_sentences += 1;
        // パターンは特殊 → 一般の順で試し、文ごとに最初に成立したものを採る
        let extracted = decompose_ja_development(sentence)
            .or_else(|| decompose_ja_possessive(sentence))
            .or_else(|| decompose_ja_copula(sentence))
            .or_else(|| decompose_en(sentence));
        if let Some(mut v) = extracted
        {
            decomposed_sentences += 1;
            triples.append(&mut v);
        }
    }

    let mut unknown_predicates: Vec<String> = Vec::new();
    for t in &triples
    {
        if predicate_class(&t.p).is_none() && !unknown_predicates.contains(&t.p)
        {
            unknown_predicates.push(t.p.clone());
        }
    }

    TripleDecomposition
    {
        success: !triples.is_empty(),
        // 脅威レビュー Medium-2: 全文が分解できた場合のみ true(未解析文が残れば false)
        fully_decomposed: total_sentences > 0 && decomposed_sentences == total_sentences,
        triples,
        unknown_predicates,
    }
}
