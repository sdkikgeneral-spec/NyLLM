# 分散セマンティックキャッシュ — PoC 最小ループ (Rust)

質問 → Embedding → 意味検索 → ヒットならキャッシュ回答 / ミスなら Agent 推論 → 署名付き登録、
という単一ノードの核ループを Rust で実装した PoC。ミス時の登録経路には S2 で判定パイプライン
(L0語彙ゲート → L2 Agent自己申告 → 案4トリプル分解 → §10.1揮発性初期付与)が入り、
共有可否を単なる語彙ルールより厳密に判定する。

設計の背景は [`docs/PoC_Design_Notes.md`](../docs/PoC_Design_Notes.md) と
[`docs/信頼性設計メモ.md`](../docs/信頼性設計メモ.md) を参照。
テスト項目・結果・動作確認は [`docs/PoC_Test_Results.md`](../docs/PoC_Test_Results.md) を参照。

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
cargo test                      # 通常テスト(71 passed / 0 failed / 2 ignored)
cargo test --features ed25519   # Ed25519 実署名でも同一テストが通ることを確認(同じく71 passed / 2 ignored)

# 検索ベンチ(#[ignore] 付き。release ビルドでないと非現実的に遅い数値になるため必ず --release で)
cargo test --release -- --ignored --nocapture bench_lookup

# §7 判定フロー通過率の実測(#[ignore] 付き。代表質問12問の固定セットで回帰assertも兼ねる)
cargo test -- --ignored --nocapture pipeline_flow_passrate
```

テストが検証している主な内容:

- `entry_id = sha256(signed_payload)` の独立再計算による照合(facts 込みの場合も含む。`test_cache_facts.rs`)
- 改ざん検知の3経路分離(answer 書き換え / author_sig 書き換え / facts 書き換え=いずれもハッシュ不一致か署名検証失敗で除外。設計メモ §4 の「ハッシュ=検知、署名=詐称防止」の区別)。`volatility_confidence` 等の署名対象外フィールドの改ざんは検知されない(意図的)ことも合わせて確認
- キャッシュの HIT/MISS 動作(しきい値)
- L0 揮発性分類(volatile/slow)と共有可否 AND ゲート(各条件の単独ブロック+全通過時のみ共有可)
- L2 Agent自己申告・案4トリプル分解・揮発性初期付与(§10.1 の4ルール)を個別に検証(`test_finalize_volatility.rs`, `test_triples.rs`)
- §7 判定パイプライン `judge_entry` の end-to-end 動作、各段が最初にブロックした段(`blocked_at`)を正しく指すこと、L2の Yes/permanent 側の申告が判定を緩めないこと(`test_pipeline.rs`)
- `DummySigner` の署名/検証ラウンドトリップ・鍵永続化・MAC としての限界

新規/拡充されたテストファイル(S2判定パイプライン): `test_triples.rs`(トリプル分解・述語オントロジー・目的語形状ガード・全角数字)、`test_finalize_volatility.rs`(§10.1の4分岐・全角時事目的語降格)、`test_pipeline.rs`(`judge_entry` end-to-end・`blocked_at`・未知述語/部分分解ブロック・全角コピュラ毒の回帰)、`test_cache_facts.rs`(facts署名不変条件・facts改ざん検知・署名対象外フィールドの非対称)、`bench_pipeline.rs`(§7通過率実測)。

読者向け注記: L0 は質問側の語彙のみを見て判定するため、質問だけでは volatile と分からない回答(例:「A社の株価は3000円です」)が最終確定段(§10.1)で回答述語由来の volatile として捕捉されることがある。レポート上では「L0 では slow だが最終 volatility は volatile」という行が出うる。また L2 の文脈依存ブロックは L0 が先に捕捉するため構造上到達せず、`blocked_at=L2SelfDeclaration` は「生成文で factual=false」経由でのみ発生する(保守的AND+L0優先の帰結であり設計どおり)。

## 構成

| ファイル | 役割 |
|---|---|
| `src/main.rs` | 通しデモ(7問 → 1 hit / 6 misses → 判定パイプライン(L0/L2/案4/確定) → 署名付き登録 → 改ざん検知) |
| `src/cache.rs` | `CacheEntry` データモデル(`facts: Vec<FactTriple>` は署名対象、`volatility_confidence`/`volatility_evidence` は署名対象外)+ `SemanticCache`(brute-force コサイン類似度検索、JSON永続化、検証付きロード) |
| `src/embedder.rs` | `Embedder` トレイト + `MockEmbedder`(文字n-gramハッシュ、決定論的、モデル不要) |
| `src/onnx_embedder.rs` | `OnnxEmbedder`(実Embedding経路の拡張点。`feature = "onnx"` 時のみコンパイル。現状は未配線のスケルトン) |
| `src/signer.rs` | `Signer` トレイト + `DummySigner`(既定) |
| `src/signer/ed25519_signer.rs` | `Ed25519Signer`(`ed25519-dalek` による実Ed25519署名、`feature = "ed25519"` 時) |
| `src/agent.rs` | `Agent` トレイト + `MockAgent`(固定回答)。`self_declare` が L2 Agent自己申告(§7.3)を返す。実LLM経路はこのトレイトに差し込む |
| `src/volatility.rs` | L0揮発性タグ(時間指示語→volatile/他→slow)+ 共有可否ANDゲート(`share_gate`)+ 揮発性初期付与の確定ロジック `finalize_volatility`(§10.1の4ルール) |
| `src/triples.rs` | 案4 知識グラフ分解: `decompose` が回答文を `(s, p, o)` へ分解、`PREDICATE_ONTOLOGY`(日英代表述語)で揮発性クラスを事前付与 |
| `src/pipeline.rs` | §7 判定パイプライン `judge_entry`: L0 → L2 → 案4分解 → 確定volatility を1本で通し、`PipelineReport`(各段の観測値・`blocked_at`)を返す |
| `src/tests/` | 単体テスト一式(`common.rs`=一時ディレクトリヘルパー、`test_cache.rs` / `test_cache_facts.rs` / `test_volatility.rs` / `test_finalize_volatility.rs` / `test_triples.rs` / `test_pipeline.rs` / `test_signer.rs`、`bench_cache.rs` / `bench_pipeline.rs`=`#[ignore]`付きベンチ)。`main.rs` の `#[cfg(test)] mod tests` から配線 |
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
- 揮発性は L0語彙ルール + L2 Agent自己申告 + 案4トリプル分解 + §10.1確定ロジックまで実装済み(S2完了)。ただし述語オントロジーは日英の代表述語のみ・完全なNLPは範囲外であり、多言語対応や語彙・数値境界の精緻化は今後の課題(詳細は [`docs/Roadmap.md`](../docs/Roadmap.md) のS2節を参照)
- S2の判定パイプラインはミス時(登録時)にのみ実行され、P2P化(S3)後に受信ノード側で共有可否を再判定する設計はまだ無い(受信側は送信側の`shareable`をそのまま信頼する前提のPoC縮約。既知の残存リスクとして [`docs/Roadmap.md`](../docs/Roadmap.md) に記録)
