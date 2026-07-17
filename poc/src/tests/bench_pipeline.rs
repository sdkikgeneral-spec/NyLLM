// §7 判定フローの通過率実測(pipeline.rs / Architecture §7 / Roadmap S2 ゲート)。
//
// 実行方法:
//   cargo test -- --ignored --nocapture pipeline_flow_passrate
//
// 注意:
//   - #[ignore] 付きなので通常の `cargo test` ではスキップされる(計測専用)。
//   - --nocapture を付けないと println! の集計結果が表示されない。
//   - これは「通過率の実測値をテスト出力として残す」ための計測であり、
//     同時に各代表質問の期待分類を assert して回帰検出も兼ねる。
//
// 計測対象: 代表質問 12 問(permanent/slow/volatile・共有可/不可・
// 分解成功/失敗が混ざる固定セット)。回答は「実LLMが返したと仮定した文」を
// 直接与え、judge_entry(question, answer, MockAgent) を1本ずつ通す。
// 各段は §7.4 の順で最初にブロックした段を blocked_at に記録する。

use crate::agent::MockAgent;
use crate::pipeline::{judge_entry, PipelineStage};

// 1 ケース = (質問, 回答, 期待共有可否, 期待blocked段, 期待volatilityクラス)。
// 期待クラスは判定の焦点になるケースのみ Some(...) とし、L0/L2 で早期に
// ブロックされる(クラスが判定の主眼でない)ケースは None にして表示のみ行う。
struct Case
{
    q: &'static str,
    a: &'static str,
    expect_shareable: bool,
    expect_blocked: Option<PipelineStage>,
    expect_class: Option<&'static str>,
}

fn cases() -> Vec<Case>
{
    vec![
        // --- 全段通過(共有可) ---
        Case { q: "日本の首都はどこですか", a: "日本の首都は東京です。",
               expect_shareable: true, expect_blocked: None, expect_class: Some("permanent") },
        Case { q: "Rustを開発したのは誰ですか", a: "Rust was developed by Mozilla.",
               expect_shareable: true, expect_blocked: None, expect_class: Some("permanent") },
        Case { q: "トヨタの本社所在地はどこですか", a: "トヨタの本社所在地は愛知県です。",
               expect_shareable: true, expect_blocked: None, expect_class: Some("slow") },
        Case { q: "What is HTTP", a: "HTTP is a protocol.",
               expect_shareable: true, expect_blocked: None, expect_class: Some("permanent") },
        // --- 案4 未知述語でブロック(脅威レビュー Medium-2: allowlist) ---
        // 述語「色」はオントロジー未収録。§10.1 ルール3で slow 分類のまま
        // (ローカル保持は可)だが、S2 の共有可は収録済み述語のみに限定するため
        // 共有不可(以前は slow=共有可だった。保守側への変更)。
        Case { q: "犬の色は何ですか", a: "犬の色は茶色です。",
               expect_shareable: false, expect_blocked: Some(PipelineStage::UnknownPredicate), expect_class: Some("slow") },
        // --- L0 語彙ゲートでブロック ---
        Case { q: "最新のClaudeモデルは何ですか", a: "最新版はClaude Opus 4です。",
               expect_shareable: false, expect_blocked: Some(PipelineStage::L0Lexical), expect_class: Some("volatile") },
        Case { q: "おすすめのエディタはどれですか", a: "おすすめはVSCodeです。",
               expect_shareable: false, expect_blocked: Some(PipelineStage::L0Lexical), expect_class: None },
        Case { q: "それについてもっと教えて", a: "はい、詳しく説明します。",
               expect_shareable: false, expect_blocked: Some(PipelineStage::L0Lexical), expect_class: None },
        Case { q: "私のコードをレビューしてください", a: "承知しました。",
               expect_shareable: false, expect_blocked: Some(PipelineStage::L0Lexical), expect_class: None },
        // --- L2 自己申告でブロック(生成文=非事実型) ---
        Case { q: "量子重力理論の展望を論じてください",
               a: "(モック回答) 「量子重力理論の展望」への回答をここでLLMが生成します。",
               expect_shareable: false, expect_blocked: Some(PipelineStage::L2SelfDeclaration), expect_class: None },
        // --- 案4 トリプル分解でブロック(事実型申告だが分解不能) ---
        Case { q: "P2Pの利点を説明してください", a: "さまざまな要因が複雑に絡み合っています。",
               expect_shareable: false, expect_blocked: Some(PipelineStage::TripleDecomposition), expect_class: None },
        // --- 確定 volatility(述語由来)でブロック ---
        Case { q: "A社の指標を教えてください", a: "A社の株価は3000円です。",
               expect_shareable: false, expect_blocked: Some(PipelineStage::FinalVolatility), expect_class: Some("volatile") },
    ]
}

#[test]
#[ignore]
fn pipeline_flow_passrate()
{
    let agent = MockAgent;
    let cases = cases();
    let n = cases.len();

    // 段別の独立通過カウント(各段を単独で見たときの通過数)。
    let mut l0_pass = 0usize;
    let mut l2_pass = 0usize;
    let mut decomp_pass = 0usize;
    let mut nonvolatile_pass = 0usize;
    let mut shareable = 0usize;

    // §7.4 ファネル: 最初にブロックした段の分布。
    let (mut b_l0, mut b_l2, mut b_tri, mut b_unk, mut b_vol, mut passed_all) =
        (0, 0, 0, 0, 0, 0);

    // 確定 volatility クラス内訳。
    let (mut c_perm, mut c_slow, mut c_vol) = (0, 0, 0);

    println!("\n=== §7 判定フロー通過率 実測(代表質問 {n} 問)===");
    println!("{:<26} {:<5} {:<5} {:<7} {:<10} {:<6} {}", "質問(先頭)", "L0", "L2", "案4", "確定class", "共有", "blocked_at");

    for c in &cases
    {
        let r = judge_entry(c.q, c.a, &agent);

        if r.l0_gate.shareable { l0_pass += 1; }
        let l2ok = r.declaration.context_independent && r.declaration.factual;
        if l2ok { l2_pass += 1; }
        if r.decomposition.success { decomp_pass += 1; }
        if r.volatility.class != "volatile" { nonvolatile_pass += 1; }
        if r.shareable { shareable += 1; }

        match r.blocked_at
        {
            None => passed_all += 1,
            Some(PipelineStage::L0Lexical) => b_l0 += 1,
            Some(PipelineStage::L2SelfDeclaration) => b_l2 += 1,
            Some(PipelineStage::TripleDecomposition) => b_tri += 1,
            Some(PipelineStage::UnknownPredicate) => b_unk += 1,
            Some(PipelineStage::FinalVolatility) => b_vol += 1,
        }

        match r.volatility.class.as_str()
        {
            "permanent" => c_perm += 1,
            "slow" => c_slow += 1,
            "volatile" => c_vol += 1,
            _ => {}
        }

        // 1行サマリ(質問は表示幅のため先頭のみ)。
        let qhead: String = c.q.chars().take(12).collect();
        println!(
            "{:<26} {:<5} {:<5} {:<7} {:<10} {:<6} {:?}",
            qhead,
            if r.l0_gate.shareable { "○" } else { "×" },
            if l2ok { "○" } else { "×" },
            if r.decomposition.success { "○" } else { "×" },
            r.volatility.class,
            if r.shareable { "可" } else { "不可" },
            r.blocked_at
        );

        // 回帰検出用アサーション(計測と同時に期待値を固定する)。
        assert_eq!(r.shareable, c.expect_shareable, "共有可否が期待と異なる: q={}", c.q);
        assert_eq!(r.blocked_at, c.expect_blocked, "blocked_at が期待と異なる: q={}", c.q);
        if let Some(cls) = c.expect_class
        {
            assert_eq!(r.volatility.class, cls, "確定クラスが期待と異なる: q={}", c.q);
        }
    }

    let pct = |x: usize| 100.0 * x as f32 / n as f32;
    println!("\n--- 段別 独立通過率(各段を単独で見た通過数 / {n})---");
    println!("L0 語彙ゲート通過 : {l0_pass}/{n} ({:.1}%)", pct(l0_pass));
    println!("L2 自己申告通過   : {l2_pass}/{n} ({:.1}%)", pct(l2_pass));
    println!("案4 分解成功      : {decomp_pass}/{n} ({:.1}%)", pct(decomp_pass));
    println!("確定 非volatile   : {nonvolatile_pass}/{n} ({:.1}%)", pct(nonvolatile_pass));
    println!("最終 共有可(全AND): {shareable}/{n} ({:.1}%)", pct(shareable));

    println!("\n--- §7.4 ファネル(最初にブロックした段の分布)---");
    println!("L0Lexical           : {b_l0}");
    println!("L2SelfDeclaration   : {b_l2}");
    println!("TripleDecomposition : {b_tri}");
    println!("UnknownPredicate    : {b_unk}");
    println!("FinalVolatility     : {b_vol}");
    println!("全段通過(共有可)   : {passed_all}");

    println!("\n--- 確定 volatility クラス内訳 ---");
    println!("permanent : {c_perm}  /  slow : {c_slow}  /  volatile : {c_vol}");

    // 集計の健全性(内訳合計 = 総数)。
    assert_eq!(b_l0 + b_l2 + b_tri + b_unk + b_vol + passed_all, n);
    assert_eq!(c_perm + c_slow + c_vol, n);
    assert_eq!(shareable, passed_all, "shareable 数と全段通過数が一致しない");
}
