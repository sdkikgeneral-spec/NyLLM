# PoC テスト項目と結果 (Rust)

> 設計ノート(スコープ/モジュール/依存/ビルド/割り切り/次ステップ): [PoC_Design_Notes.md](./PoC_Design_Notes.md)
> 関連: [Winny_Type_Semantic_Cache_Architecture.md](./Winny_Type_Semantic_Cache_Architecture.md) / [信頼性設計メモ.md](./Winny_Type_Semantic_Cache_信頼性設計メモ.md) / [Roadmap.md](./Roadmap.md)
> 実装: `poc/`(Rust / Cargo)
> 作成日: 2026-07-16
> 更新: 2026-07-17 S2判定パイプライン(L2 Agent自己申告/案4トリプル分解/揮発性初期付与§10.1/§7フローパイプライン)のテスト項目・実測を追加
> 更新: 2026-07-17 旧 `PoC_Implementation.md`(さらに旧称 `PoC_Minimal_Loop.md`)から設計ノートを [`PoC_Design_Notes.md`](./PoC_Design_Notes.md) へ分離し、本ファイルをテスト項目・結果・動作確認に純化してファイル名を `PoC_Test_Results.md` に変更

---

## 1. 対象と前提

本ドキュメントは `poc/`(単一ノード実装)の **テスト項目・結果・動作確認** をまとめた報告書であり、**S1(最小ループ)+ S2(判定パイプライン)** を対象とする。設計の背景・モジュール構成・割り切りは [`PoC_Design_Notes.md`](./PoC_Design_Notes.md) を参照。

単体テストは `poc/src/tests/` に配置(`common.rs` は一時ディレクトリヘルパー)。`main.rs` に `#[cfg(test)] mod tests` を配線(`#[path]` で `src/tests/` 配下を参照。production ロジックは不変)。実行コマンドの詳細は [`poc/README.md`](../poc/README.md) の「テスト」節を参照(本節では重複記載しない)。

## 2. 実測サマリ(S1+S2合算、2026-07-17 時点、Windows 11 / Rust 1.97 stable-msvc)

| コマンド | 結果 |
|---|---|
| `cargo test` | **71 passed / 0 failed / 2 ignored** |
| `cargo test --features ed25519` | **71 passed / 0 failed / 2 ignored**(署名テストは DummySigner 直接利用のため feature 有無に非依存) |
| `cargo run` | 7問デモで regression なし(1 hit / 6 misses / キャッシュ6件、改ざん1件書換→再読込後5件) |

`ignored` の2件はいずれも `#[ignore]` 付きベンチマーク(`bench_lookup` / `pipeline_flow_passrate`)で、通常の `cargo test` ではスキップされる(§6)。

## 3. 動作確認(エンドツーエンドのデモ)

`main.rs` は「質問6問 → hit/miss → 登録 → 改ざん検知」を通しで実演するデモであり、フルループの確認手段はこのバイナリ実行のまま。7問のデモ(`cargo run`。1 hit / 6 misses)で以下を確認:

- 「Winnyとは何ですか?」初回 → MISS → MockAgent回答 → `slow`/共有可 → 署名付き登録
- 同一質問の再質問 → **HIT (sim=1.000, 検索1μs)**
- 「Winnyって何?」(言い換え)/「P2Pの仕組みを教えてください」→ MISS(Mock Embedderは意味理解しないため言い換えヒットは出ない)
- 「日本の首都はどこですか?」→ 案4トリプル分解(所有格パターン)で `(日本, 首都, 東京)` を抽出 → 述語オントロジーで `首都`=permanent型 → 全段通過で `permanent`/共有可
- 「最新のClaudeのモデルは何ですか?」→ L0で時間指示語検出 → `volatile` 判定 → **共有不可(ローカル短期TTLのみ)**
- 「おすすめのエディタは?」→ L0で主観語検出 → **共有不可**
- 改ざん検知: 保存済みJSONの `answer` を書き換え → 再ロード時に hash 不一致で**当該エントリのみ除外**

S2で追加された判定パイプライン(§7.3/§10.1)の§7フロー通過率の実測は代表質問12問の固定セットで別途行っている(§6 `pipeline_flow_passrate`)。再現方法・実測値は [`docs/Roadmap.md`](./Roadmap.md) のS2節を参照。

## 4. S1(PoC最小ループ)のテスト

**`test_cache.rs`(対象: `cache.rs`)** — 7件

| テスト関数 | 検証内容 |
|---|---|
| `entry_id_equals_sha256_of_signed_payload` | `entry_id` が `signed_payload()` の sha256(hex) と一致する(ハッシュ計算はテスト側で独立に再現) |
| `valid_entry_survives_reload` | 改ざんしていない正常エントリは再ロードでも生き残る(ポジティブコントロール) |
| `tampered_answer_dropped_by_hash_mismatch` | 保存済みJSONの `answer` 書き換え → 再計算ハッシュが `entry_id` と不一致で除外(改ざん**検知**の経路) |
| `tampered_signature_dropped_by_verify_failure` | `author_sig` のみ書き換え(内容ハッシュは一致のまま)→ 署名検証失敗で除外(詐称**防止**の経路) |
| `exact_match_hits_with_similarity_near_one` | 完全一致質問で HIT(sim ≥ 0.999) |
| `empty_cache_misses` | 空キャッシュでは MISS(`entry` が `None`) |
| `dissimilar_query_misses_below_threshold` | 無関係な質問は `LOCAL_THRESHOLD` 未満で MISS |

改ざん系の2テストは、設計メモ §4 の「ハッシュ=改ざん検知」「署名=詐称防止」が**別々の検知経路**であることを個別に実証する意図で分離している(`author_sig` は `signed_payload` に含まれないため、書き換えてもハッシュ照合は通り、署名検証だけが失敗する)。

**`test_volatility.rs`(対象: `volatility.rs`)** — 7件

| テスト関数 | 検証内容 |
|---|---|
| `classify_time_referring_question_is_volatile` | 時間指示語(「最新」)を含む質問 → `volatile` |
| `classify_plain_question_is_slow` | 時間指示語を含まない事実質問 → `slow` |
| `context_dependent_question_blocks_share` | 文脈依存語(「それ」)**単独**で共有不可 |
| `subjective_question_blocks_share` | 主観語(best)**単独**で共有不可 |
| `personal_question_blocks_share` | 個人参照(「私の」)**単独**で共有不可 |
| `volatile_alone_blocks_share` | 中立質問でも `volatility=="volatile"` **単独**で共有不可 |
| `all_clear_factual_slow_is_shareable` | 全ブロック条件をクリアした場合**のみ** `shareable=true`(ANDゲートの唯一の可ケース) |

**`test_signer.rs`(対象: `signer.rs` / `DummySigner`)** — 6件

| テスト関数 | 検証内容 |
|---|---|
| `sign_then_verify_roundtrip_succeeds` | 自ノード鍵での署名→検証ラウンドトリップ成功 |
| `verify_fails_for_different_payload` | ペイロード改ざんで検証失敗 |
| `verify_fails_for_tampered_signature` | 署名文字列改ざんで検証失敗 |
| `verify_fails_across_different_keys` | 別鍵の signer では検証不可(公開検証できない **MAC の限界**の実証) |
| `same_key_file_reloads_and_verifies` | 同一鍵ファイルから再読込した別インスタンス間で検証成功(鍵の永続化) |
| `creates_missing_multi_level_parent_dirs` | 多階層の未存在親ディレクトリを持つ鍵パスでも鍵ファイルが自動生成される |

これらは `DummySigner` を直接使うため、`feature = "ed25519"` の有無に依存せず通る。

## 5. S2(判定パイプライン)のテスト

**`test_cache_facts.rs`(対象: `cache.rs` の facts 経路)** — 3件

| テスト関数 | 検証内容 |
|---|---|
| `judged_entry_id_equals_sha256_including_facts` | facts を含むエントリでも `entry_id = sha256(signed_payload)`(facts込み)が成立する |
| `tampered_fact_dropped_by_hash_mismatch` | 保存済みJSONの `facts[0].o` を書き換え → 再計算ハッシュ不一致で除外(改ざん**検知**の第3経路) |
| `tampered_volatility_confidence_is_not_signed` | 署名対象外の `volatility_confidence` を書き換えてもエントリは除外されない(設計どおりの非対称性の確認) |

**`test_finalize_volatility.rs`(対象: `volatility.rs::finalize_volatility`。§10.1)** — 12件

§10.1「初期ルール」の4分岐(ルール1: 分解成功+全permanent型述語→permanent、ルール2: 時間指示語→強制volatile最優先・質問/回答両方走査、ルール3: 分解失敗/未知述語→slowデフォルト、ルール4: 自己申告は不一致時にconfidence低下のみ)をそれぞれ単独検証。加えてコピュラ定義述語(種別/is-a)の目的語形状ガード(半角/全角の数値・通貨・時点語・年を含む目的語はpermanent昇格を抑止しvolatileへ降格)を正例・負例(定義語のみ・英字に挟まれた数字は降格しない)双方で検証(脅威レビューMedium-1対応の回帰含む)。

**`test_triples.rs`(対象: `triples.rs::decompose`/`PREDICATE_ONTOLOGY`。案4)** — 21件

日本語(開発文/所有格/コピュラ定義文)・英語(developed by/"the P of S is O"/is-a)の各分解パターン、述語オントロジーの照合(canonical/alias、大文字小文字)、疑問文・引用符断片の棄却(生成文の誤抽出防止)、目的語の時事シグナル判定(半角/全角数字、通貨、時点語、年)を検証。

**`test_pipeline.rs`(対象: `pipeline.rs::judge_entry`。§7)** — 15件

全段通過(共有可)、L0/L2/案4分解不能/案4未知述語(allowlist)/案4部分分解(fully_decomposed=false)/確定volatilityの各段が単独でブロックし `blocked_at` が§7.4の順で最初のブロック段を正しく指すこと、全角コピュラ毒(脅威再レビュー回帰)、L2のYes/permanent側申告が判定を緩める経路が存在しないことを横断的に検証。

## 6. ベンチマーク(`#[ignore]` 付き)

いずれも通常の `cargo test` ではスキップされる。debug ビルドでは非現実的に遅い数値になるため、計測時は必ず `--release` を付けること。**下記は断定的な性能保証ではなく環境依存の参考計測値**であり、必要に応じて再計測すること。

**`bench_cache.rs::bench_lookup`(対象: `SemanticCache::lookup()`)**

`SemanticCache::lookup()` を n=100 / 1,000 / 10,000 件のシンセティックエントリに対し各100回呼び、平均時間を計測。

実行コマンド: `cargo test --release -- --ignored --nocapture bench_lookup`

| n(件) | lookup 平均 |
|---|---|
| 100 | 約 46 µs/回 |
| 1,000 | 約 454 µs/回 |
| 10,000 | 約 5,387 µs/回 |

ほぼ O(n) 線形(設計ノート §2 の brute-force 走査どおり)。

**`bench_pipeline.rs::pipeline_flow_passrate`(対象: `pipeline.rs::judge_entry`。§7)**

代表質問12問の固定セットに対し `judge_entry` を通し、段別独立通過率・§7.4ファネル(最初にブロックした段の分布)・確定volatilityクラス内訳を計測。回帰assertも兼ねる。実測値は [`docs/Roadmap.md`](./Roadmap.md) のS2節を参照。

## 7. ロードマップとの対応

本テスト整備をもって S1(PoC最小ループ)のテストゲートを通過。2026-07-17時点では、S2判定パイプライン(L2自己申告/案4トリプル分解/揮発性初期付与/§7フローパイプライン)の実装・テスト・脅威モデルレビューも完了し、§7フロー通過率の実測というS2のゲート条件も達成している。進捗ステータス・実測値・S3着手前に必須の残存リスクの一次情報は [`docs/Roadmap.md`](./Roadmap.md) を参照。
