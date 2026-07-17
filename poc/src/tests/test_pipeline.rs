// 判定パイプライン judge_entry のテスト(pipeline.rs / Architecture §7)。
//
// 検証対象:
//   - 全段通過(文脈自立 かつ 事実型 かつ 非volatile)でのみ shareable=true
//   - 各段が単独で共有をブロックし、blocked_at が §7.4 の判定順で
//     「最初にブロックした段」を正しく指すこと
//   - L2 自己申告の Yes/事実型 側が L0・案4・確定volatility の判定を
//     緩める経路が存在しないこと(安全側のみ反映)
//
// judge_entry(question, answer, agent) は answer を引数で受け取るため、
// ここでは「実LLMが返したと仮定した回答」を直接与えて各段を独立に駆動する。
// (MockAgent.self_declare は answer/question の語彙から申告を導出する)

use crate::agent::MockAgent;
use crate::pipeline::{judge_entry, PipelineStage};

// ------------------------------------------------------------------
// 全段通過(共有可)
// ------------------------------------------------------------------

#[test]
fn all_stages_pass_is_shareable()
{
    // 文脈自立・事実型・permanent(非volatile)・分解成功 → 全段通過で共有可。
    let agent = MockAgent;
    let r = judge_entry("日本の首都はどこですか", "日本の首都は東京です。", &agent);
    assert!(r.shareable, "全段通過のはずが共有不可: {}", r.share_reason);
    assert_eq!(r.blocked_at, None, "共有可なのに blocked_at が設定されている");
    assert_eq!(r.volatility.class, "permanent");
    assert!(r.decomposition.success);
    // 共有可であることは全ての肯定条件が揃っていることを含意する。
    assert!(r.l0_gate.shareable);
    assert!(r.declaration.context_independent);
    assert!(r.declaration.factual);
}

#[test]
fn english_factual_permanent_is_shareable()
{
    // 英語の事実型 permanent 回答も全段通過する。
    let agent = MockAgent;
    let r = judge_entry("What is HTTP", "HTTP is a protocol.", &agent);
    assert!(r.shareable, "英語事実文が共有可にならなかった: {}", r.share_reason);
    assert_eq!(r.blocked_at, None);
    assert_eq!(r.volatility.class, "permanent");
}

// ------------------------------------------------------------------
// L0 語彙ゲートでのブロック(最初の段)
// ------------------------------------------------------------------

#[test]
fn l0_blocks_on_time_term()
{
    // 質問に時間指示語("最新") → L0 で volatile 判定されブロック。
    let agent = MockAgent;
    let r = judge_entry("最新のClaudeモデルは何ですか", "最新のClaudeモデルはClaude Opus 4です。", &agent);
    assert!(!r.shareable);
    assert_eq!(r.blocked_at, Some(PipelineStage::L0Lexical), "時間指示語が L0 でブロックされなかった");
}

#[test]
fn l0_blocks_on_context_term()
{
    // 文脈依存語("それ") → L0 ブロック。
    let agent = MockAgent;
    let r = judge_entry("それについてもっと教えて", "はい、詳しく説明します。", &agent);
    assert!(!r.shareable);
    assert_eq!(r.blocked_at, Some(PipelineStage::L0Lexical));
}

#[test]
fn l0_blocks_on_subjective_term()
{
    // 主観語("おすすめ") → L0 ブロック。
    let agent = MockAgent;
    let r = judge_entry("おすすめのエディタはどれですか", "おすすめはVSCodeです。", &agent);
    assert!(!r.shareable);
    assert_eq!(r.blocked_at, Some(PipelineStage::L0Lexical));
}

#[test]
fn l0_blocks_on_personal_term()
{
    // 個人参照("私の") → L0 ブロック。
    let agent = MockAgent;
    let r = judge_entry("私のコードをレビューしてください", "承知しました。", &agent);
    assert!(!r.shareable);
    assert_eq!(r.blocked_at, Some(PipelineStage::L0Lexical));
}

// ------------------------------------------------------------------
// L2 自己申告でのブロック
// ------------------------------------------------------------------

#[test]
fn l2_blocks_non_factual_generated_answer()
{
    // L0 は通過する中立質問だが、回答が生成文(MockAgent フォールバック形式)。
    // MockAgent.self_declare が factual=false を申告 → L2 でブロック。
    // blocked_at は案4(TripleDecomposition)より前の L2SelfDeclaration を指す。
    let agent = MockAgent;
    let answer = "(モック回答) 「量子重力理論の展望」への回答をここでLLMが生成します。";
    let r = judge_entry("量子重力理論の展望を論じてください", answer, &agent);
    assert!(!r.shareable);
    assert_eq!(
        r.blocked_at,
        Some(PipelineStage::L2SelfDeclaration),
        "生成文が L2(事実型でない)でブロックされなかった"
    );
    assert!(!r.declaration.factual, "前提: 生成文は factual=false のはず");
}

// ------------------------------------------------------------------
// 案4 トリプル分解でのブロック
// ------------------------------------------------------------------

#[test]
fn triple_decomposition_blocks_undecomposable_factual_answer()
{
    // L0 通過・L2 通過(事実型かつ文脈自立と申告)だが、回答が分解不能。
    // → 案4(TripleDecomposition)でブロック。§7.4 の順で L2 の後に来る段。
    let agent = MockAgent;
    let r = judge_entry("P2Pの利点を説明してください", "さまざまな要因が複雑に絡み合っています。", &agent);
    assert!(!r.shareable);
    assert_eq!(
        r.blocked_at,
        Some(PipelineStage::TripleDecomposition),
        "分解不能な回答が案4でブロックされなかった"
    );
    // 安全側のみ反映の実証: L2 申告は肯定的(単独回答可・事実型)なのに、
    // 分解不能という客観信号でブロックされている(Yes 申告が緩めない)。
    assert!(r.declaration.context_independent, "前提: 文脈自立と申告しているはず");
    assert!(r.declaration.factual, "前提: 事実型と申告しているはず");
    assert!(!r.decomposition.success, "前提: 分解は失敗しているはず");
}

// ------------------------------------------------------------------
// 確定 volatility(述語由来)でのブロック
// ------------------------------------------------------------------

#[test]
fn final_volatility_blocks_predicate_derived_volatile()
{
    // 質問に時間指示語はない(L0 は slow で通過)が、回答の述語が volatile 型。
    // → 案4 は分解成功、確定 volatility が volatile になり FinalVolatility でブロック。
    // L0(質問のみ)では捕捉できない揮発性を確定段が捕捉する経路。
    // 注: ルール2の回答側走査(脅威レビュー Medium-1)が先に発火しないよう、
    // 述語「順位」(オントロジー収録の volatile 型だが L0 語彙表には無い)を使い、
    // 述語クラス由来の volatile 判定そのものを検証する。
    let agent = MockAgent;
    let r = judge_entry("A社の順位を教えてください", "A社の順位は3位です。", &agent);
    assert_eq!(r.l0_volatility, "slow", "前提: 質問側 L0 は slow のはず");
    assert!(r.decomposition.success, "前提: 分解は成功しているはず");
    assert_eq!(r.volatility.class, "volatile", "述語由来の volatile が確定していない");
    assert!(!r.shareable);
    assert_eq!(
        r.blocked_at,
        Some(PipelineStage::FinalVolatility),
        "述語由来 volatile が確定段でブロックされなかった"
    );
}

#[test]
fn final_volatility_blocks_fullwidth_copula_poison()
{
    // 脅威再レビュー Medium(全角数字汚染)回帰テスト。
    // 「レートは１５０です」型(全角時事値を持つコピュラ定義文)は、修正前は
    // is_timely_object が全角数字を取りこぼし、種別=permanent へ洗浄されて
    // 共有可(汚染)になりうる経路が残っていた。全角対応後は目的語形状ガードが
    // 全角数値を拾い、permanent 昇格を抑止 → volatile 降格で共有不可になる。
    //
    // 質問は時間中立(L0 通過)・回答は生成文でない(L2 通過)・分解成功かつ
    // 全文分解・収録済み述語(種別)なので、共有不可の理由は確定 volatility のみ
    // (blocked_at=FinalVolatility)であることを確認する。
    let agent = MockAgent;
    let r = judge_entry("ドル円レートはいくらですか", "レートは１５０です。", &agent);
    // 先行する段が全て通過していること(全角値以外で落ちていない=ガードが効いた証拠)。
    assert!(r.l0_gate.shareable, "前提: L0 は通過するはず");
    assert!(r.declaration.context_independent && r.declaration.factual, "前提: L2 は通過するはず");
    assert!(r.decomposition.success && r.decomposition.fully_decomposed, "前提: 全文分解できるはず");
    assert!(r.decomposition.unknown_predicates.is_empty(), "前提: 種別は収録済み述語のはず");
    // 全角時事目的語で volatile へ降格し、共有不可になっていること。
    assert_eq!(r.volatility.class, "volatile", "全角時事目的語が volatile へ降格しなかった(permanent 洗浄の疑い)");
    assert!(!r.shareable, "全角時事値のコピュラ定義文が共有可になった(汚染)");
    assert_eq!(
        r.blocked_at,
        Some(PipelineStage::FinalVolatility),
        "全角時事値が確定 volatility 段でブロックされなかった"
    );
    // 降格根拠が目的語形状ガード由来(全角目的語)であること。
    assert!(
        r.volatility.evidence.iter().any(|e| e == "timely_definition_object:１５０"),
        "全角目的語の形状ガード evidence が無い: {:?}",
        r.volatility.evidence
    );
}

// ------------------------------------------------------------------
// 案4 未知述語 allowlist でのブロック(脅威レビュー Medium-2)
// ------------------------------------------------------------------

#[test]
fn unknown_predicate_blocks_share_but_class_stays_slow()
{
    // 「犬の色は茶色です」の述語「色」はオントロジー未収録。
    //   - 揮発性クラスは §10.1 ルール3で slow のまま(ローカル保持は可)
    //   - S2 の共有可は収録済み述語のみの allowlist なので共有不可
    //     (blocked_at=UnknownPredicate。共有フラグのみ保守側へ倒す)
    let agent = MockAgent;
    let r = judge_entry("犬の色は何ですか", "犬の色は茶色です。", &agent);
    // L0/L2/分解成功/fully_decomposed は全て通過する(未知述語だけが理由)。
    assert!(r.l0_gate.shareable, "前提: L0 は通過するはず");
    assert!(r.declaration.context_independent && r.declaration.factual, "前提: L2 は通過するはず");
    assert!(r.decomposition.success, "前提: 分解は成功するはず");
    assert!(r.decomposition.fully_decomposed, "前提: 全文分解できるはず");
    // 未知述語が記録され、UnknownPredicate 段でブロックされる。
    assert!(
        r.decomposition.unknown_predicates.contains(&"色".to_string()),
        "未知述語が記録されていない: {:?}",
        r.decomposition.unknown_predicates
    );
    assert!(!r.shareable, "未知述語エントリが共有可になった");
    assert_eq!(
        r.blocked_at,
        Some(PipelineStage::UnknownPredicate),
        "未知述語が UnknownPredicate 段でブロックされなかった"
    );
    // 揮発性クラスは slow のまま(共有可否と分離されている)。
    assert_eq!(r.volatility.class, "slow", "未知述語でクラスまで動いてしまった(slow のはず)");
}

#[test]
fn known_predicate_only_entry_is_shareable()
{
    // 対照: 収録済み述語のみ(本社所在地=slow)のエントリは従来どおり共有可。
    // 未知述語ブロックが「未知述語だけ」を落としていること(過剰にブロックしない)の確認。
    let agent = MockAgent;
    let r = judge_entry("トヨタの本社所在地はどこですか", "トヨタの本社所在地は愛知県です。", &agent);
    assert!(r.decomposition.unknown_predicates.is_empty(), "前提: 未知述語は無いはず");
    assert!(r.shareable, "収録済み述語のみのエントリが共有不可になった: {}", r.share_reason);
    assert_eq!(r.blocked_at, None);
    assert_eq!(r.volatility.class, "slow");
}

// ------------------------------------------------------------------
// 案4 部分分解(fully_decomposed=false)でのブロック(脅威レビュー Medium-2)
// ------------------------------------------------------------------

#[test]
fn partial_decomposition_blocks_at_triple_stage()
{
    // 1文目は分解成功(首都)だが 2文目が分解不能 → success=true・fully_decomposed=false。
    // 未解析文の内容を検証できないため共有除外(blocked_at=TripleDecomposition)。
    // success=true なので「分解不能(success=false)」ではなく fully_decomposed 分岐で落ちる。
    let agent = MockAgent;
    let r = judge_entry("日本の地理について教えてください", "日本の首都は東京です。多くの利用者がいる。", &agent);
    assert!(r.l0_gate.shareable, "前提: L0 は通過するはず");
    assert!(r.declaration.context_independent && r.declaration.factual, "前提: L2 は通過するはず");
    assert!(r.decomposition.success, "前提: 1文以上は分解できているはず");
    assert!(!r.decomposition.fully_decomposed, "前提: 未解析文が残っているはず");
    assert!(!r.shareable, "部分分解エントリが共有可になった");
    assert_eq!(
        r.blocked_at,
        Some(PipelineStage::TripleDecomposition),
        "部分分解が TripleDecomposition 段でブロックされなかった"
    );
}

#[test]
fn fully_decomposed_multi_sentence_is_shareable()
{
    // 対照: 複数文が全て分解でき(全て収録済み述語)、時事シグナルもない場合は共有可。
    // fully_decomposed ブロックが「未解析文が残るときだけ」効くことの確認。
    let agent = MockAgent;
    let r = judge_entry(
        "日本とトヨタについて教えてください",
        "日本の首都は東京です。トヨタの本社所在地は愛知県です。",
        &agent,
    );
    assert!(r.decomposition.success && r.decomposition.fully_decomposed, "前提: 全文分解できるはず");
    assert!(r.decomposition.unknown_predicates.is_empty(), "前提: 未知述語は無いはず");
    assert!(r.shareable, "全文分解できたエントリが共有不可になった: {}", r.share_reason);
    assert_eq!(r.blocked_at, None);
}

// ------------------------------------------------------------------
// 安全側のみ反映(緩める経路の不在)を横断的に確認
// ------------------------------------------------------------------

#[test]
fn shareable_implies_all_positive_signals()
{
    // shareable=true は「全ての肯定条件が揃っている」と同値でなければならない。
    // ブロックされたどのケースでも、いずれか一つ以上の否定条件が立っている。
    let agent = MockAgent;
    let cases: &[(&str, &str)] = &[
        ("最新のClaudeモデルは何ですか", "最新版はClaude Opus 4です。"),
        ("それについてもっと教えて", "はい。"),
        ("おすすめのエディタはどれですか", "VSCodeです。"),
        ("A社の指標を教えてください", "A社の株価は3000円です。"),
        ("P2Pの利点を説明してください", "さまざまな要因が絡みます。"),
    ];
    for (q, a) in cases
    {
        let r = judge_entry(q, a, &agent);
        // ブロックされたなら blocked_at は必ず Some、shareable は false。
        assert!(!r.shareable, "共有不可のはずが可: q={q}");
        assert!(r.blocked_at.is_some(), "共有不可なのに blocked_at が None: q={q}");
    }
}
