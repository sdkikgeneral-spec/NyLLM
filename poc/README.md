# Winny型 Semantic Cache — PoC 最小ループ (C++)

質問 → Embedding → 意味検索 → ヒットならキャッシュ回答 / ミスなら Agent 推論 → 署名付き登録、
という単一ノードの核ループを C++17 で実装した PoC。

設計の背景は [`docs/PoC_Minimal_Loop.md`](../docs/PoC_Minimal_Loop.md) と
[`docs/Winny_Type_Semantic_Cache_信頼性設計メモ.md`](../docs/Winny_Type_Semantic_Cache_信頼性設計メモ.md) を参照。

## ビルドと実行

外部依存ゼロ(既定構成)。必要なのは [Meson](https://mesonbuild.com/) と Ninja、C++17 コンパイラのみ。

```sh
meson setup build
meson compile -C build

# 実行
./build/semantic_cache_poc          # Linux / macOS
.\build\semantic_cache_poc.exe      # Windows

# 既存キャッシュを保持したまま実行
./build/semantic_cache_poc --keep
```

Meson / Ninja が PATH に無い場合は pip 版でも動く(グローバルインストール不要):

```sh
python -m pip install meson ninja
python -m mesonbuild.mesonmain setup build
python -m mesonbuild.mesonmain compile -C build
```

Windows では Visual Studio(MSVC)を Meson が自動検出する(VS 同梱の Ninja も利用可)。
`meson compile` は MSVC 環境を自動でアクティベートするため、開発者コマンドプロンプトは必須ではない。

実行するとカレントディレクトリに `cache_store/`(エントリJSON)と `keys/node.key`(ノード鍵)が作られる。

## 構成

| ファイル | 役割 |
|---|---|
| `src/main.cpp` | 通しデモ(6問 → ヒット/ミス → 登録 → 改ざん検知) |
| `src/cache.hpp` | `CacheEntry` データモデル + `SemanticCache`(brute-force コサイン類似度検索、JSON永続化、検証付きロード) |
| `src/embedder.hpp` | `IEmbedder` + `MockEmbedder`(文字n-gramハッシュ、決定論的、モデル不要) |
| `src/onnx_embedder.hpp` | `OnnxEmbedder`(ONNX Runtime 経路の拡張点。`POC_USE_ONNX` 時のみコンパイル) |
| `src/signer.hpp` | `ISigner` + `DummySigner`(既定) / `SodiumSigner`(Ed25519、`POC_USE_SODIUM` 時) |
| `src/agent.hpp` | `IAgent` + `MockAgent`(固定回答)。実LLM経路はこのIFに差し込む |
| `src/volatility.hpp` | L0揮発性タグ(時間指示語→volatile/他→slow)+ 共有可否ANDゲート |
| `vendor/sha256.hpp` | 依存ゼロのSHA-256(content hash / ダミーMAC用) |
| `vendor/nlohmann/json.hpp` | JSON永続化(単一ヘッダ、同梱) |
| `python_prototype/` | 初期Pythonプロトタイプ(参考。実Embedding/実Claude Agentの実装例あり) |

## Meson オプション(依存の有無で変わること)

| オプション | 既定 | ON にすると |
|---|---|---|
| `use_sodium` | false | 署名が `DummySigner`(sha256鍵付きMAC=公開検証不可のプレースホルダ)から libsodium の **Ed25519 実署名** に切り替わる |
| `use_onnx` | false | `OnnxEmbedder`(ONNX Runtime)がコンパイルされる。※トークナイザ未統合のため現状は骨格のみ。既定の `MockEmbedder` は表記類似度ベースの擬似Embedding |

```sh
meson setup build -Duse_sodium=true   # 要 libsodium
meson setup build -Duse_onnx=true     # 要 onnxruntime
# 既存の build を再構成する場合は configure:
meson configure build -Duse_sodium=true
```

いずれも false のままで全機能(検索・登録・署名フロー・改ざん検知)が動作する。

## しきい値

- ローカル利用: `kLocalThreshold = 0.80`(MeanCache の MPNet τ=0.83 前後の知見)
- 共有想定: `kSharedThreshold = 0.90`(誤ヒットは人類規模で汚染が広がるため精度優先)

## PoC の割り切り

- P2P・witness署名・ノード評判・失効(revocation)は対象外(単一ノード)
- 検索は線形走査 O(n)。数万件規模までは十分。将来 faiss/HNSW に差し替える拡張点
- `MockEmbedder` は意味理解をしない(表記が近い質問のみ高類似度)。完全一致は sim=1.0 でヒットするが、言い換えは実モデル(ONNX経路)が必要
- `DummySigner` は MAC であり詐称防止にならない。署名インターフェースとフローの実証用。実運用は `POC_USE_SODIUM=ON`
- 揮発性は L0 語彙ルールのみ。permanent 昇格(知識グラフ分解=案4)は未実装
