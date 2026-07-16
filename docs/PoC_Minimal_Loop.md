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
