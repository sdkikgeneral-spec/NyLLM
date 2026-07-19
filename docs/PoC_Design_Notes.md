# PoC実装 — 設計ノート (Rust)

> 親ドキュメント: [AI_Concept.md](./AI_Concept.md) /
> [信頼性設計メモ.md](./信頼性設計メモ.md)
> テスト項目・結果・動作確認: [PoC_Test_Results.md](./PoC_Test_Results.md)
> 実装: `poc/`(Rust / Cargo)
> 作成日: 2026-07-16
> 更新: 2026-07-16 実装をC++からRustへ移行(理由: P2P化以降、悪意あるピア由来の未検証データをパースする箇所が増える。メモリ安全性がそのままリモート悪用可能なバグの有無に直結するため)
> 更新: 2026-07-17 S2判定パイプライン(L2 Agent自己申告/案4トリプル分解/揮発性初期付与§10.1/§7フローパイプライン)の実装完了を反映(データモデル・割り切りを更新)
> 更新: 2026-07-17 設計ノートとテスト報告を分割。旧 `PoC_Implementation.md`(さらに旧称 `PoC_Minimal_Loop.md`)をテスト報告 [`PoC_Test_Results.md`](./PoC_Test_Results.md) に純化し、設計ノート(スコープ/モジュール/依存/ビルド/割り切り/次ステップ)を本ファイルへ分離
> 更新: 2026-07-17 S2.5(エントリ形式再設計)の実装完了を反映(データモデル・改ざん検知・永続化レイアウトを不変コア/可変状態分離後の形に更新。詳細は [`Roadmap.md`](./Roadmap.md) §0[B]・[`S2.5_エントリ形式設計.md`](./S2.5_エントリ形式設計.md))

---

## 1. スコープ

本ドキュメントは `poc/`(単一ノード実装)の設計ノートであり、**S1(最小ループ)+ S2(判定パイプライン)+ S2.5(エントリ形式再設計)** を対象とする。P2P・評判・失効・witness分散などS3以降の分散化はこれまで通り**引き続きscope外**であり、扱わない(S2.5でPhase2空スロットとして型の場所だけ確保した`witness_sigs`/`anchor_proof`/`stake`/`trust`も、中身の実装はscope外のまま)。動作確認結果・テスト項目は [`PoC_Test_Results.md`](./PoC_Test_Results.md) を参照。

```text
質問 → Embedding生成 → 意味検索(コサイン類似度)
  ├ ヒット(類似度 >= しきい値) → キャッシュ回答を返す
  └ ミス → Agent(LLM)へ推論委譲 → 回答生成
              → 判定パイプライン(§7): L0語彙 → L2自己申告 → 案4トリプル分解 → §10.1揮発性確定
              → 署名付きでキャッシュ登録(entry_id=hex(sha256(core_bytes))、facts込み。S2.5) → 返す
```

意味検索・キャッシュ照合はホットパスであり性能要件があるため、実装言語は **Rust**(初期Pythonプロトタイプは `poc/python_prototype/` に退避)。

## 2. 構成とモジュール

| モジュール | 実装 | 設計メモとの対応 |
|---|---|---|
| Embedding | `Embedder` トレイト。既定 `MockEmbedder`(文字n-gramハッシュ→512次元→L2正規化、決定論的・モデル不要)。オプション `OnnxEmbedder`(実Embedding経路の拡張点、`feature = "onnx"`。現状は未配線のスケルトン) | §1: MPNet級モデル推奨 → 実運用は実Embedding経路。PoCは依存ゼロ優先 |
| 意味検索 | 全エントリとの内積(正規化済みなのでコサイン類似度)を brute-force 走査 | PoC規模でO(n)十分。ANN系クレートへの差し替え拡張点 |
| しきい値 | `LOCAL_THRESHOLD=0.80` / `SHARED_THRESHOLD=0.90` | §1(MeanCache τ≈0.83)・§2脅威A(共有は精度優先で0.9+) |
| データモデル | **S2.5(2026-07-17実装完了)で不変コア/可変状態に分離**。`ImmutableCore { schema_ver, question_norm, facts, provenance{agent, model, embedder_model_id}, created, initial_volatility_class, initial_tier }`(署名・entry_id対象、浮動小数点なし)+ `MutableState { volatility_class_operative, volatility_confidence, volatility_evidence, shareable, share_reason, tier_operative, local_embedder_id, trust, witness_sigs, anchor_proof, stake }`(署名対象外・ノードローカル導出。末尾4フィールドはPhase2空スロットで常に空)。インメモリ `CacheEntry` はこの2つ+`entry_id`/`question_key`/`core_bytes`/`author_pub`/`author_sig`/`embedding`(非保存・ロード時再計算)を束ねる。(**追記 2026-07-20**: `src/core` 版の `MutableState` には受信由来判別の `origin_received: bool`〔署名対象外・wire非搭載・ノードローカル・既定`true`=保守側。共有由来エントリへの `SHARED_THRESHOLD` 適用の判別用〕が追加されたが、**凍結済みの `poc/` には存在しない**。→ [S2.5_エントリ形式設計.md](./S2.5_エントリ形式設計.md) §2・[Architecture.md](./Architecture.md) §7[補完]「既知の穴」) | Architecture §6スキーマの縮小(witness_sigs/trust/anchor_proof/stakeは型のみ確保する空スロット、中身はPhase2)。`provenance.agent`はS2.5で署名対象化済み(旧: 未署名) |
| 改ざん検知 | `entry_id = hex(sha256(core_bytes))`。`core_bytes` は serde 非依存の長さ接頭辞バイナリ(ドメインタグ `nyllm/entry/v1\n` 先頭・フィールド固定順、詳細は [S2.5_エントリ形式設計.md](./S2.5_エントリ形式設計.md) §1・§3)。`question_key = hex(sha256(qkey_bytes))` を別ドメインタグ `nyllm/qkey/v1\n` で算出し検索・重複排除・版束ねを担う。ロード時に `core_bytes` を再計算しファイル名と照合 | Architecture §6「ハッシュ=改ざん検知」 |
| 詐称防止 | `Signer` トレイト。既定 `DummySigner`(HMAC-SHA256鍵付きMAC=プレースホルダ。S2.5で旧`sha256(secret‖payload)`から長さ拡張攻撃対策として変更)、`feature = "ed25519"` で `ed25519-dalek` による **Ed25519 実署名** | Architecture §8.5「署名=誰が言ったかの固定」 |
| Agent | `Agent` トレイト。既定 `MockAgent`(固定回答、ネット不要)。`self_declare` がL2自己申告(§7.3)を返す。実LLM(Claude等)は同トレイトへの差し込み拡張点(HTTP依存を必須にしない) | Agent層の抽象化 + Architecture §7.3 L2ゲート |
| 判定パイプライン | `pipeline::judge_entry` がL0語彙 → L2自己申告 → 案4トリプル分解(`triples::decompose`+`PREDICATE_ONTOLOGY`) → §10.1揮発性確定(`volatility::finalize_volatility`)を1本で通す(S2で実装完了) | Architecture §7全体 / §10.1 |
| 揮発性タグ | L0ルール(時間指示語(最新/現在/今日/latest…)→`volatile`/それ以外→`slow`)+ §10.1の4ルールで確定(分解成功かつ全述語permanent型→permanent、時間指示語→強制volatile最優先、分解失敗/未知述語→slowデフォルト、自己申告は不一致時にconfidence低下のみ) | Architecture §7.3, §10.1「疑わしきはslow/volatile側へ」 |
| 共有ゲート | ANDゲート: L0(文脈依存語・主観語・個人参照・volatileのいずれかを含む→不可)に加え、L2自己申告(context_independent/factual)・案4分解成功+全文分解済み+全述語オントロジー収録済み・確定非volatileを全て満たした場合のみ共有可。デフォルト非共有 | Architecture §7.1「文脈自立 × 事実型」保守的デフォルト |
| 永続化 | 1エントリ=2ファイル: `cache_store/<entry_id>.entry`(不変。serde JSONエンベロープ`{schema_ver, core_b64, author_pub, author_sig}`、authoritativeなのは中の`core_b64`のみ)+ `cache_store/<entry_id>.state.json`(可変。`MutableState`、ノードローカル・署名なし)。ロード時に `core_bytes` のhash+署名を検証し不一致は読み捨てた上で、`judge_entry` を再実行して `shareable`/`tier_operative`/`volatility_class_operative` を再導出(送信者側の値・`state.json`の値は信頼しない、§6の10手順) | 毒エントリの構造的排除(信頼性設計メモ §2脅威C の最小版)+ 受信側再導出による偽装`shareable`の無効化(S2.5) |

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
6. 判定パイプライン(§7)は登録時(`judge_entry`)に加え、**S2.5(2026-07-17実装完了)以降はロード時にも再導出**を行い、`shareable`/`tier_operative`/`volatility_class_operative`を送信者側の値を信頼せず再導出する(§6の10手順。実装形は `cache.rs::derive_operative_state`: core は answer 平文を保存しないため`judge_entry`全段〔L2自己申告・`decompose`再実行を含む〕の文字どおりの再実行ではなく、署名済み `facts` からの運用値再導出である。旧: 受信側再判定の仕組みが無くS3着手前必須の残存リスクだったが解消済み。詳細は [`Roadmap.md`](./Roadmap.md) §0[B])。ただしP2P配布経路自体(受信という概念)は本PoCには存在せず、単一ノード内でのreloadのみで検証している点は引き続きPoC縮約(reload時の非単調性が既知の妥協点として残る。[S2.5_エントリ形式設計.md](./S2.5_エントリ形式設計.md) §13 High-1)。なおノード間配送・受信側検証はS3実装(2026-07-18、`src/core`+`registry`。→ [`Roadmap.md`](./Roadmap.md) §2「S3 P2P化」節)で実現済みだが、`poc/` はS1/S2参照実装として凍結されており本ノートの記述対象は `poc/` のまま

## 6. 次のステップ候補

- 実Embedding経路のトークナイザ+推論バックエンド統合(`tokenizers`クレート + multilingual-MiniLM、推論は`ort`クレート等)で言い換えヒットを実証
- `ed25519`featureを既定化し DummySigner を排除
- volatile エントリの TTL 失効(created + TTL で読み時破棄)
- 2ノード間のエントリ交換シミュレーション(witness署名の最小実装。`MutableState.witness_sigs`はS2.5でPhase2空スロットとして型のみ確保済み、中身はS3で実装)。受信側での共有可否再判定・embeddingの再計算・provenance署名化はS2.5で解消済みのため、S3ではこれに加え `decompose` の入力長・反復回数上限(DoS対策)・question_keyあたりの版数上限/レート制限(詳細は [`docs/Roadmap.md`](./Roadmap.md) §3未解決事項・S2.5節、[S2.5_エントリ形式設計.md](./S2.5_エントリ形式設計.md) §13)を必須要件とする。**追記(2026-07-20、再スコープ)**: このうち `decompose` の入力長・反復回数上限は、S3実装(2026-07-18、`src/core`)が受信answerを再分解しない設計(Transferはcore+署名のみを運び、受信側は署名済み`facts`から`derive_operative_state`で再導出)になったため**再スコープ(前提陳腐化)**となった — 完全対応による解消ではなく、受信側が自由文answerを再分解する設計へ変えた場合に再浮上する条件付き課題(登録時`decompose`の全体入力長・文数上限の不在は自ノードAgent出力のため優先度低の残タスク)。詳細は [`docs/Roadmap.md`](./Roadmap.md) §3の該当行。版数上限/レート制限の方はS3実装でも未実装のままPhase1既知制約として残っている(Roadmap §2 S3節)
