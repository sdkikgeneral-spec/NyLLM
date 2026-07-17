# PoC実装 — 設計ノート (Rust)

> 親ドキュメント: [AI_Concept.md](./AI_Concept.md) /
> [信頼性設計メモ.md](./信頼性設計メモ.md)
> テスト項目・結果・動作確認: [PoC_Test_Results.md](./PoC_Test_Results.md)
> 実装: `poc/`(Rust / Cargo)
> 作成日: 2026-07-16
> 更新: 2026-07-16 実装をC++からRustへ移行(理由: P2P化以降、悪意あるピア由来の未検証データをパースする箇所が増える。メモリ安全性がそのままリモート悪用可能なバグの有無に直結するため)
> 更新: 2026-07-17 S2判定パイプライン(L2 Agent自己申告/案4トリプル分解/揮発性初期付与§10.1/§7フローパイプライン)の実装完了を反映(データモデル・割り切りを更新)
> 更新: 2026-07-17 設計ノートとテスト報告を分割。旧 `PoC_Implementation.md`(さらに旧称 `PoC_Minimal_Loop.md`)をテスト報告 [`PoC_Test_Results.md`](./PoC_Test_Results.md) に純化し、設計ノート(スコープ/モジュール/依存/ビルド/割り切り/次ステップ)を本ファイルへ分離

---

## 1. スコープ

本ドキュメントは `poc/`(単一ノード実装)の設計ノートであり、**S1(最小ループ)+ S2(判定パイプライン)** を対象とする。P2P・評判・失効・witness分散などS3以降の分散化はこれまで通り**引き続きscope外**であり、扱わない。動作確認結果・テスト項目は [`PoC_Test_Results.md`](./PoC_Test_Results.md) を参照。

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

詳細は [`poc/README.md`](../poc/README.md)。テストの実行方法・結果は [`PoC_Test_Results.md`](./PoC_Test_Results.md) を参照。

## 5. PoCの割り切り(既知の限界)

1. `MockEmbedder` は表記類似度のみ。「Winnyって何?」のような言い換えは sim≈0.58 でヒットしない(実モデルが必要な部分を正直に可視化)
2. `DummySigner` は公開検証不可の MAC。署名フローの実証用であり、詐称防止の実効性は `--features ed25519` で得る
3. 揮発性は L0語彙ルール+L2 Agent自己申告+案4トリプル分解+§10.1確定ロジックまでS2で実装済み。ただし述語オントロジーは日英の代表述語のみ(多言語対応・語彙/数値境界の精緻化は今後の課題)
4. TTL失効・witness・評判・revocation・regurgitationフィルタは P2P フェーズの課題
5. 検索は線形走査。大規模化時は ANN系クレート + PCA圧縮(§1のMeanCache知見)に差し替え
6. 判定パイプライン(§7)はミス時(登録時)にのみ動く。受信ノード側で共有可否を再判定する仕組みはまだ無く、`shareable`フィールドを信頼する前提のPoC縮約(S3着手前に必須の残存リスクとして [`docs/Roadmap.md`](./Roadmap.md) に記録)

## 6. 次のステップ候補

- 実Embedding経路のトークナイザ+推論バックエンド統合(`tokenizers`クレート + multilingual-MiniLM、推論は`ort`クレート等)で言い換えヒットを実証
- `ed25519`featureを既定化し DummySigner を排除
- volatile エントリの TTL 失効(created + TTL で読み時破棄)
- 2ノード間のエントリ交換シミュレーション(witness署名の最小実装)。実装時は受信側での共有可否再判定(`shareable`を信頼せず再導出)・embeddingの署名対象化/再計算・provenance署名化を必須要件とする(詳細は [`docs/Roadmap.md`](./Roadmap.md) S2節の残存リスクを参照)
