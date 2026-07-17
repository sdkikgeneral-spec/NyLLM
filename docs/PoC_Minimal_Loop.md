# PoC 最小ループ — 設計と実行方法 (Rust)

> 親ドキュメント: [Winny_Type_Semantic_Cache_AI_Concept.md](./Winny_Type_Semantic_Cache_AI_Concept.md) /
> [Winny_Type_Semantic_Cache_信頼性設計メモ.md](./Winny_Type_Semantic_Cache_信頼性設計メモ.md)
> 実装: `poc/`(Rust / Cargo)
> 作成日: 2026-07-16
> 更新: 2026-07-16 実装をC++からRustへ移行(理由: P2P化以降、悪意あるピア由来の未検証データをパースする箇所が増える。メモリ安全性がそのままリモート悪用可能なバグの有無に直結するため)

---

## 1. スコープ

P2P・評判・失効・witness分散は扱わない。**単一ノードで動く核ループだけ**を実装する。

```text
質問 → Embedding生成 → 意味検索(コサイン類似度)
  ├ ヒット(類似度 >= しきい値) → キャッシュ回答を返す
  └ ミス → Agent(LLM)へ推論委譲 → 回答生成
              → 揮発性タグ付与(L0) → 共有可否ゲート
              → 署名付きでキャッシュ登録(ID=content hash) → 返す
```

意味検索・キャッシュ照合はホットパスであり性能要件があるため、実装言語は **Rust**(初期Pythonプロトタイプは `poc/python_prototype/` に退避)。

## 2. 構成とモジュール

| モジュール | 実装 | 設計メモとの対応 |
|---|---|---|
| Embedding | `Embedder` トレイト。既定 `MockEmbedder`(文字n-gramハッシュ→512次元→L2正規化、決定論的・モデル不要)。オプション `OnnxEmbedder`(実Embedding経路の拡張点、`feature = "onnx"`。現状は未配線のスケルトン) | §1: MPNet級モデル推奨 → 実運用は実Embedding経路。PoCは依存ゼロ優先 |
| 意味検索 | 全エントリとの内積(正規化済みなのでコサイン類似度)を brute-force 走査 | PoC規模でO(n)十分。ANN系クレートへの差し替え拡張点 |
| しきい値 | `LOCAL_THRESHOLD=0.80` / `SHARED_THRESHOLD=0.90` | §1(MeanCache τ≈0.83)・§2脅威A(共有は精度優先で0.9+) |
| データモデル | `CacheEntry { question, answer, embedding, created, volatility, shareable, share_reason, agent, author_pub, author_sig, entry_id }` | §4スキーマの縮小(witness_sigs省略) |
| 改ざん検知 | `entry_id = sha256(質問+回答+日付+揮発性 の正規化JSON)` をファイル名/IDに。ロード時に再計算照合 | §4「ハッシュ=改ざん検知」 |
| 詐称防止 | `Signer` トレイト。既定 `DummySigner`(sha256鍵付きMAC=プレースホルダ)、`feature = "ed25519"` で `ed25519-dalek` による **Ed25519 実署名** | §4「署名=誰が言ったかの固定」 |
| Agent | `Agent` トレイト。既定 `MockAgent`(固定回答、ネット不要)。実LLM(Claude等)は同トレイトへの差し込み拡張点(HTTP依存を必須にしない) | Agent層の抽象化 |
| 揮発性タグ | L0ルール: 時間指示語(最新/現在/今日/latest…)→ `volatile` / それ以外 → `slow`。permanent昇格は未実装 | §3, §7「疑わしきはslow/volatile側へ」 |
| 共有ゲート | ANDゲート: 文脈依存語・主観語・個人参照のいずれかを含む、または volatile → 共有不可。デフォルト非共有 | §5「文脈自立 × 事実型」保守的デフォルト |
| 永続化 | 1エントリ=1 JSONファイル(`cache_store/<entry_id>.json`)。ロード時に hash+署名を検証し、不一致は読み捨て | 毒エントリの構造的排除(§2脅威C の最小版) |

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

6問のデモで以下を確認:

- 「Winnyとは何ですか?」初回 → MISS → MockAgent回答 → `slow`/共有可 → 署名付き登録
- 同一質問の再質問 → **HIT (sim=1.000, 検索1μs)**
- 「最新のClaudeのモデルは何ですか?」→ `volatile` 判定 → **共有不可(ローカル短期TTLのみ)**
- 「おすすめのエディタは?」→ 主観語検出 → **共有不可**
- 改ざん検知: 保存済みJSONの `answer` を書き換え → 再ロード時に hash 不一致で**当該エントリのみ除外**(5件→4件)

## 6. PoCの割り切り(既知の限界)

1. `MockEmbedder` は表記類似度のみ。「Winnyって何?」のような言い換えは sim≈0.58 でヒットしない(実モデルが必要な部分を正直に可視化)
2. `DummySigner` は公開検証不可の MAC。署名フローの実証用であり、詐称防止の実効性は `--features ed25519` で得る
3. 揮発性 L0 のみ。permanent 昇格・事実トリプル分解(案4)・L2 Agent自己申告は未実装
4. TTL失効・witness・評判・revocation・regurgitationフィルタは P2P フェーズの課題
5. 検索は線形走査。大規模化時は ANN系クレート + PCA圧縮(§1のMeanCache知見)に差し替え

## 7. 次のステップ候補

- 実Embedding経路のトークナイザ+推論バックエンド統合(`tokenizers`クレート + multilingual-MiniLM、推論は`ort`クレート等)で言い換えヒットを実証
- `ed25519`featureを既定化し DummySigner を排除
- volatile エントリの TTL 失効(created + TTL で読み時破棄)
- 2ノード間のエントリ交換シミュレーション(witness署名の最小実装)

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

**`bench_cache.rs`** — `#[ignore]` 付きベンチマーク 1件

| テスト関数 | 内容 |
|---|---|
| `bench_lookup` | `SemanticCache::lookup()` を n=100 / 1,000 / 10,000 件のシンセティックエントリに対し各100回呼び、平均時間を計測。通常の `cargo test` ではスキップされる |

### 8.2 実測結果(2026-07-17 時点)

| コマンド | 結果 |
|---|---|
| `cargo test` | **20 passed / 0 failed / 1 ignored** |
| `cargo test --features ed25519` | **20 passed / 0 failed / 1 ignored**(署名テストは DummySigner 直接利用のため feature 有無に非依存) |
| `cargo run` | 既存6問デモに regression なし(1 hit / 5 misses / キャッシュ5件、改ざん1件書換→再読込後4件) |

検索ベンチ(`cargo test --release -- --ignored --nocapture bench_lookup`)の参考計測値:

| n(件) | lookup 平均 |
|---|---|
| 100 | 約 46 µs/回 |
| 1,000 | 約 454 µs/回 |
| 10,000 | 約 5,387 µs/回 |

ほぼ O(n) 線形(§2 の brute-force 走査どおり)。**これは断定的な性能保証ではなく環境依存の参考計測値**であり、必要に応じて上記コマンドで再計測すること(debug ビルドでは非現実的に遅い数値になるため必ず `--release` を付ける)。

### 8.3 ロードマップとの対応

本テスト整備をもって S1(PoC最小ループ)のテストゲートを通過。進捗ステータスの一次情報は [`docs/Roadmap.md`](./Roadmap.md) を参照。
