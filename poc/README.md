# Winny型 Semantic Cache — PoC 最小ループ (Rust)

質問 → Embedding → 意味検索 → ヒットならキャッシュ回答 / ミスなら Agent 推論 → 署名付き登録、
という単一ノードの核ループを Rust で実装した PoC。

設計の背景は [`docs/PoC_Minimal_Loop.md`](../docs/PoC_Minimal_Loop.md) と
[`docs/Winny_Type_Semantic_Cache_信頼性設計メモ.md`](../docs/Winny_Type_Semantic_Cache_信頼性設計メモ.md) を参照。

## ビルドと実行

必要なのは [Rust](https://www.rust-lang.org/tools/install)(Cargo)のみ。既定構成の依存は
`serde` / `serde_json` / `sha2` / `hex` / `rand` / `chrono` の小さな純Rustクレートのみ。

```sh
cd poc
cargo build
cargo run

# 既存キャッシュを保持したまま実行
cargo run -- --keep
```

実行するとカレントディレクトリに `cache_store/`(エントリJSON)と `keys/node.key`(ノード鍵)が作られる。

## テスト

```sh
cd poc
cargo test                      # 通常テスト(20 passed / 1 ignored)
cargo test --features ed25519   # Ed25519 実署名でも同一テストが通ることを確認

# 検索ベンチ(#[ignore] 付き。release ビルドでないと非現実的に遅い数値になるため必ず --release で)
cargo test --release -- --ignored --nocapture bench_lookup
```

テストが検証している主な内容:

- `entry_id = sha256(signed_payload)` の独立再計算による照合
- 改ざん検知の2経路分離(answer 書き換え=ハッシュ不一致 / author_sig 書き換え=署名検証失敗。設計メモ §4 の「ハッシュ=検知、署名=詐称防止」の区別)
- キャッシュの HIT/MISS 動作(しきい値)
- 揮発性分類(volatile/slow)と共有可否 AND ゲート(各条件の単独ブロック+全通過時のみ共有可)
- `DummySigner` の署名/検証ラウンドトリップ・鍵永続化・MAC としての限界

## 構成

| ファイル | 役割 |
|---|---|
| `src/main.rs` | 通しデモ(6問 → ヒット/ミス → 登録 → 改ざん検知) |
| `src/cache.rs` | `CacheEntry` データモデル + `SemanticCache`(brute-force コサイン類似度検索、JSON永続化、検証付きロード) |
| `src/embedder.rs` | `Embedder` トレイト + `MockEmbedder`(文字n-gramハッシュ、決定論的、モデル不要) |
| `src/onnx_embedder.rs` | `OnnxEmbedder`(実Embedding経路の拡張点。`feature = "onnx"` 時のみコンパイル。現状は未配線のスケルトン) |
| `src/signer.rs` | `Signer` トレイト + `DummySigner`(既定) |
| `src/signer/ed25519_signer.rs` | `Ed25519Signer`(`ed25519-dalek` による実Ed25519署名、`feature = "ed25519"` 時) |
| `src/agent.rs` | `Agent` トレイト + `MockAgent`(固定回答)。実LLM経路はこのトレイトに差し込む |
| `src/volatility.rs` | L0揮発性タグ(時間指示語→volatile/他→slow)+ 共有可否ANDゲート |
| `src/tests/` | 単体テスト一式(`common.rs`=一時ディレクトリヘルパー、`test_cache.rs` / `test_volatility.rs` / `test_signer.rs`、`bench_cache.rs`=`#[ignore]`付きベンチ)。`main.rs` の `#[cfg(test)] mod tests` から配線 |
| `python_prototype/` | 初期Pythonプロトタイプ(参考。実Embedding/実Claude Agentの実装例あり) |

## Cargo feature(依存の有無で変わること)

| feature | 既定 | ON にすると |
|---|---|---|
| `ed25519` | off | 署名が `DummySigner`(sha256鍵付きMAC=公開検証不可のプレースホルダ)から `ed25519-dalek` の **Ed25519 実署名** に切り替わる |
| `onnx` | off | `OnnxEmbedder` がコンパイルされる。※トークナイザ・ONNX Runtimeバインディング(`ort`クレート等)とも未配線のため現状は骨格のみ。既定の `MockEmbedder` は表記類似度ベースの擬似Embedding |

```sh
cargo build --features ed25519
cargo build --features onnx
```

いずれもoffのままで全機能(検索・登録・署名フロー・改ざん検知)が動作する。

## しきい値

- ローカル利用: `LOCAL_THRESHOLD = 0.80`(MeanCache の MPNet τ=0.83 前後の知見)
- 共有想定: `SHARED_THRESHOLD = 0.90`(誤ヒットは人類規模で汚染が広がるため精度優先)

## PoC の割り切り

- P2P・witness署名・ノード評判・失効(revocation)は対象外(単一ノード)
- 検索は線形走査 O(n)。数万件規模までは十分。将来 ANN系クレートに差し替える拡張点
- `MockEmbedder` は意味理解をしない(表記が近い質問のみ高類似度)。完全一致は sim=1.0 でヒットするが、言い換えは実モデル(実Embedding経路)が必要
- `DummySigner` は MAC であり詐称防止にならない。署名インターフェースとフローの実証用。実運用は `--features ed25519`
- 揮発性は L0 語彙ルールのみ。permanent 昇格(知識グラフ分解=案4)は未実装
