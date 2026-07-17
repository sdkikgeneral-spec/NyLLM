# PoC 最小ループ — 設計と実行方法 (Rust)

> 親ドキュメント: [Winny_Type_Semantic_Cache_AI_Concept.md](./Winny_Type_Semantic_Cache_AI_Concept.md) /
> [Winny_Type_Semantic_Cache_信頼性設計メモ.md](./Winny_Type_Semantic_Cache_信頼性設計メモ.md)
> 実装: `poc/`(Rust / Cargo)
> 作成日: 2026-07-16
> 更新: 2026-07-16 実装をC++からRustへ移行(理由: P2P化以降、悪意あるピア由来の未検証データをパースする箇所が増える。メモリ安全性がそのままリモート悪用可能なバグの有無に直結するため)
> 更新: 2026-07-17 S2判定パイプライン(L2 Agent自己申告/案4トリプル分解/揮発性初期付与§10.1/§7フローパイプライン)の実装完了を反映(データモデル・デモ・テスト項目を更新)

---

## 1. スコープ

P2P・評判・失効・witness分散は扱わない。**単一ノードで動く核ループだけ**を実装する。

```text
質問 → Embedding生成 → 意味検索(コサイン類似度)
  ├ ヒット(類似度 >= しきい値) → キャッシュ回答を返す
  └ ミス → Agent(LLM)へ推論委譲 → 回答生成
              → 判定パイプライン(§7): L0語彙 → L2自己申告 → 案4トリプル分解 → §10.1揮発性確定
              → 署名付きでキャッシュ登録(ID=content hash、facts込み) → 返す
```

意味検索・キャッシュ照合はホットパスであり性能要件があるため、実装言語は **Rust**(初期Pythonプロトタイプは `poc/python_prototype/` に退避)。

## 2. 構成とモジュール

| モジュール | 実装 | 設計メモとの対応 |
|---|---|---|
| Embedding | `Embedder` トレイト。既定 `MockEmbedder`(文字n-gramハッシュ→512次元→L2正規化、決定論的・モデル不要)。オプション `OnnxEmbedder`(実Embedding経路の拡張点、`feature = "onnx"`。現状は未配線のスケルトン) | §1: MPNet級モデル推奨 → 実運用は実Embedding経路。PoCは依存ゼロ優先 |
| 意味検索 | 全エントリとの内積(正規化済みなのでコサイン類似度)を brute-force 走査 | PoC規模でO(n)十分。ANN系クレートへの差し替え拡張点 |
| しきい値 | `LOCAL_THRESHOLD=0.80` / `SHARED_THRESHOLD=0.90` | §1(MeanCache τ≈0.83)・§2脅威A(共有は精度優先で0.9+) |
| データモデル | `CacheEntry { question, answer, embedding, created, volatility, volatility_confidence, volatility_evidence, facts, shareable, share_reason, agent, author_pub, author_sig, entry_id }`。`facts: Vec<{s,p,o}>` は署名対象、`volatility_confidence`/`volatility_evidence` は再評価で更新される可変推定値のため署名対象外(§10) | Architecture §6スキーマの縮小(witness_sigs・provenanceの署名対象化を省略) |
| 改ざん検知 | `entry_id = sha256(質問+回答+日付+揮発性クラス+事実トリプル の正規化JSON)` をファイル名/IDに。ロード時に再計算照合 | Architecture §6「ハッシュ=改ざん検知」 |
| 詐称防止 | `Signer` トレイト。既定 `DummySigner`(sha256鍵付きMAC=プレースホルダ)、`feature = "ed25519"` で `ed25519-dalek` による **Ed25519 実署名** | Architecture §8.5「署名=誰が言ったかの固定」 |
| Agent | `Agent` トレイト。既定 `MockAgent`(固定回答、ネット不要)。`self_declare` がL2自己申告(§7.3)を返す。実LLM(Claude等)は同トレイトへの差し込み拡張点(HTTP依存を必須にしない) | Agent層の抽象化 + Architecture §7.3 L2ゲート |
| 判定パイプライン | `pipeline::judge_entry` がL0語彙 → L2自己申告 → 案4トリプル分解(`triples::decompose`+`PREDICATE_ONTOLOGY`) → §10.1揮発性確定(`volatility::finalize_volatility`)を1本で通す(S2で実装完了) | Architecture §7全体 / §10.1 |
| 揮発性タグ | L0ルール(時間指示語(最新/現在/今日/latest…)→`volatile`/それ以外→`slow`)+ §10.1の4ルールで確定(分解成功かつ全述語permanent型→permanent、時間指示語→強制volatile最優先、分解失敗/未知述語→slowデフォルト、自己申告は不一致時にconfidence低下のみ) | Architecture §7.3, §10.1「疑わしきはslow/volatile側へ」 |
| 共有ゲート | ANDゲート: L0(文脈依存語・主観語・個人参照・volatileのいずれかを含む→不可)に加え、L2自己申告(context_independent/factual)・案4分解成功+全文分解済み+全述語オントロジー収録済み・確定非volatileを全て満たした場合のみ共有可。デフォルト非共有 | Architecture §7.1「文脈自立 × 事実型」保守的デフォルト |
| 永続化 | 1エントリ=1 JSONファイル(`cache_store/<entry_id>.json`)。ロード時に hash+署名を検証し、不一致は読み捨て | 毒エントリの構造的排除(信頼性設計メモ §2脅威C の最小版) |

## 3. 依存方針

- **既定構成の依存は小さな純Rustクレートのみ**(`serde`/`serde_json`/`sha2`/`hex`/`rand`/`chrono`。Cargoが自動解決するため vendor/ 同梱は不要)。
- 重量級依存は Cargo feature で任意有効化:
  - `--features ed25519` → Ed25519 実署名(`ed25519-dalek`)
  - `--features onnx` → 実Embedding経路の拡張点(現状は未配線のスケルトン。トークナイザ・ONNX Runtimeバインディングは今後)
- どちらの feature も無くても Mock 経路で通しビルド・実行できる。

## 4. ビルド・実行

```sh
cd poc
cargo build
cargo run
```

詳細は [`poc/README.md`](../poc/README.md)。

## 5. 動作確認結果(Mock経路 / Windows 11、Rust 1.97 stable-msvc)

7問のデモ(`cargo run`。1 hit / 6 misses)で以下を確認:

- 「Winnyとは何ですか?」初回 → MISS → MockAgent回答 → `slow`/共有可 → 署名付き登録
- 同一質問の再質問 → **HIT (sim=1.000, 検索1μs)**
- 「Winnyって何?」(言い換え)/「P2Pの仕組みを教えてください」→ MISS(Mock Embedderは意味理解しないため言い換えヒットは出ない)
- 「日本の首都はどこですか?」→ 案4トリプル分解(所有格パターン)で `(日本, 首都, 東京)` を抽出 → 述語オントロジーで `首都`=permanent型 → 全段通過で `permanent`/共有可
- 「最新のClaudeのモデルは何ですか?」→ L0で時間指示語検出 → `volatile` 判定 → **共有不可(ローカル短期TTLのみ)**
- 「おすすめのエディタは?」→ L0で主観語検出 → **共有不可**
- 改ざん検知: 保存済みJSONの `answer` を書き換え → 再ロード時に hash 不一致で**当該エントリのみ除外**

S2で追加された判定パイプライン(§7.3/§10.1)の§7フロー通過率の実測は代表質問12問の固定セットで別途行っている。再現方法・実測値は [`docs/Roadmap.md`](./Roadmap.md) のS2節を参照。

## 6. PoCの割り切り(既知の限界)

1. `MockEmbedder` は表記類似度のみ。「Winnyって何?」のような言い換えは sim≈0.58 でヒットしない(実モデルが必要な部分を正直に可視化)
2. `DummySigner` は公開検証不可の MAC。署名フローの実証用であり、詐称防止の実効性は `--features ed25519` で得る
3. 揮発性は L0語彙ルール+L2 Agent自己申告+案4トリプル分解+§10.1確定ロジックまでS2で実装済み。ただし述語オントロジーは日英の代表述語のみ(多言語対応・語彙/数値境界の精緻化は今後の課題)
4. TTL失効・witness・評判・revocation・regurgitationフィルタは P2P フェーズの課題
5. 検索は線形走査。大規模化時は ANN系クレート + PCA圧縮(§1のMeanCache知見)に差し替え
6. 判定パイプライン(§7)はミス時(登録時)にのみ動く。受信ノード側で共有可否を再判定する仕組みはまだ無く、`shareable`フィールドを信頼する前提のPoC縮約(S3着手前に必須の残存リスクとして [`docs/Roadmap.md`](./Roadmap.md) に記録)

## 7. 次のステップ候補

- 実Embedding経路のトークナイザ+推論バックエンド統合(`tokenizers`クレート + multilingual-MiniLM、推論は`ort`クレート等)で言い換えヒットを実証
- `ed25519`featureを既定化し DummySigner を排除
- volatile エントリの TTL 失効(created + TTL で読み時破棄)
- 2ノード間のエントリ交換シミュレーション(witness署名の最小実装)。実装時は受信側での共有可否再判定(`shareable`を信頼せず再導出)・embeddingの署名対象化/再計算・provenance署名化を必須要件とする(詳細は [`docs/Roadmap.md`](./Roadmap.md) S2節の残存リスクを参照)

## 8. テスト項目と結果

単体テストは `poc/src/tests/` に配置(`common.rs` は一時ディレクトリヘルパー)。実行コマンドの詳細は [`poc/README.md`](../poc/README.md) の「テスト」節を参照(本節では重複記載しない)。

### 8.1 テストファイルと検証内容

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

**`test_cache_facts.rs`(対象: `cache.rs` の facts 経路。S2で追加)** — 3件

| テスト関数 | 検証内容 |
|---|---|
| `judged_entry_id_equals_sha256_including_facts` | facts を含むエントリでも `entry_id = sha256(signed_payload)`(facts込み)が成立する |
| `tampered_fact_dropped_by_hash_mismatch` | 保存済みJSONの `facts[0].o` を書き換え → 再計算ハッシュ不一致で除外(改ざん**検知**の第3経路) |
| `tampered_volatility_confidence_is_not_signed` | 署名対象外の `volatility_confidence` を書き換えてもエントリは除外されない(設計どおりの非対称性の確認) |

**`test_finalize_volatility.rs`(対象: `volatility.rs::finalize_volatility`。§10.1。S2で追加)** — 12件

§10.1「初期ルール」の4分岐(ルール1: 分解成功+全permanent型述語→permanent、ルール2: 時間指示語→強制volatile最優先・質問/回答両方走査、ルール3: 分解失敗/未知述語→slowデフォルト、ルール4: 自己申告は不一致時にconfidence低下のみ)をそれぞれ単独検証。加えてコピュラ定義述語(種別/is-a)の目的語形状ガード(半角/全角の数値・通貨・時点語・年を含む目的語はpermanent昇格を抑止しvolatileへ降格)を正例・負例(定義語のみ・英字に挟まれた数字は降格しない)双方で検証(脅威レビューMedium-1対応の回帰含む)。

**`test_triples.rs`(対象: `triples.rs::decompose`/`PREDICATE_ONTOLOGY`。案4。S2で追加)** — 21件

日本語(開発文/所有格/コピュラ定義文)・英語(developed by/"the P of S is O"/is-a)の各分解パターン、述語オントロジーの照合(canonical/alias、大文字小文字)、疑問文・引用符断片の棄却(生成文の誤抽出防止)、目的語の時事シグナル判定(半角/全角数字、通貨、時点語、年)を検証。

**`test_pipeline.rs`(対象: `pipeline.rs::judge_entry`。§7。S2で追加)** — 15件

全段通過(共有可)、L0/L2/案4分解不能/案4未知述語(allowlist)/案4部分分解(fully_decomposed=false)/確定volatilityの各段が単独でブロックし `blocked_at` が§7.4の順で最初のブロック段を正しく指すこと、全角コピュラ毒(脅威再レビュー回帰)、L2のYes/permanent側申告が判定を緩める経路が存在しないことを横断的に検証。

**`bench_cache.rs`** — `#[ignore]` 付きベンチマーク 1件

| テスト関数 | 内容 |
|---|---|
| `bench_lookup` | `SemanticCache::lookup()` を n=100 / 1,000 / 10,000 件のシンセティックエントリに対し各100回呼び、平均時間を計測。通常の `cargo test` ではスキップされる |

**`bench_pipeline.rs`** — `#[ignore]` 付きベンチマーク 1件(S2で追加)

| テスト関数 | 内容 |
|---|---|
| `pipeline_flow_passrate` | 代表質問12問の固定セットに対し `judge_entry` を通し、段別独立通過率・§7.4ファネル(最初にブロックした段の分布)・確定volatilityクラス内訳を計測。回帰assertも兼ねる。実測値は [`docs/Roadmap.md`](./Roadmap.md) のS2節を参照 |

### 8.2 実測結果(2026-07-17 時点)

| コマンド | 結果 |
|---|---|
| `cargo test` | **71 passed / 0 failed / 2 ignored** |
| `cargo test --features ed25519` | **71 passed / 0 failed / 2 ignored**(署名テストは DummySigner 直接利用のため feature 有無に非依存) |
| `cargo run` | 7問デモで regression なし(1 hit / 6 misses / キャッシュ6件、改ざん1件書換→再読込後5件) |

検索ベンチ(`cargo test --release -- --ignored --nocapture bench_lookup`)の参考計測値:

| n(件) | lookup 平均 |
|---|---|
| 100 | 約 46 µs/回 |
| 1,000 | 約 454 µs/回 |
| 10,000 | 約 5,387 µs/回 |

ほぼ O(n) 線形(§2 の brute-force 走査どおり)。**これは断定的な性能保証ではなく環境依存の参考計測値**であり、必要に応じて上記コマンドで再計測すること(debug ビルドでは非現実的に遅い数値になるため必ず `--release` を付ける)。

### 8.3 ロードマップとの対応

本テスト整備をもって S1(PoC最小ループ)のテストゲートを通過。2026-07-17時点では、S2判定パイプライン(L2自己申告/案4トリプル分解/揮発性初期付与/§7フローパイプライン)の実装・テスト・脅威モデルレビューも完了し、§7フロー通過率の実測というS2のゲート条件も達成している。進捗ステータス・実測値・S3着手前に必須の残存リスクの一次情報は [`docs/Roadmap.md`](./Roadmap.md) を参照。
