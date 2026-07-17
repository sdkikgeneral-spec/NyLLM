# Winny型 Semantic Cache — 実装ロードマップ

> 出典: 本ファイルは [Winny_Type_Semantic_Cache_Architecture.md](./Winny_Type_Semantic_Cache_Architecture.md) 旧§13.2「実装ステップ」+ 旧§13.1「未解決事項」+ 旧§13.3「PoC最小ループ(S1詳細)」を抽出・統合したもの。
> 二重管理回避のため、**実装ステップの内容(段階/ゲート)と進捗ステータスの一次情報はこのファイルのみ**とする。Architecture.md 側は参照リンクのみを残す。
> 前提ドキュメント: [Architecture.md](./Winny_Type_Semantic_Cache_Architecture.md) / [信頼性設計メモ.md](./Winny_Type_Semantic_Cache_信頼性設計メモ.md) / [PoC_Design_Notes.md](./PoC_Design_Notes.md) / [PoC_Test_Results.md](./PoC_Test_Results.md)
> 作成日: 2026-07-17
> ステータス凡例: **未着手** / **一部着手**(範囲の一部のみ着手・段階の目標としては未達) / **一部完了**(主要機能は動くが残作業あり) / **完了**(ゲート通過)
> ステータスは `poc/`(および将来の `src/core`, `src/ui`)の実ソースを確認して記載している。裏取り方法は各段階の節を参照。

---

## 1. 実装ステップ全体表 (S1〜S7)

| 段階 | 内容 | ゲート(通過条件) | 現状ステータス |
|---|---|---|---|
| S1 PoC最小ループ | Embedding検索 → ミス時Agent呼び出し → 署名付きキャッシュ登録(単一ノード) | Hit/Missが動く | **完了** |
| S2 判定パイプライン | L0/L2ゲート+案4トリプル分解+揮発性初期付与 | §7フロー通過率を実測 | **完了** |
| S3 P2P化 | DHT分散・witness署名・複数版併存 | 2ノード以上で共有 | **未着手** |
| S4 評判・独立検証 | 3層評判・スラッシング・層3抜き打ち | 毒注入テストに耐える | **未着手** |
| S5 法的機構 | regurgitationフィルタ・revocationフラッディング・出所記録 | 失効が全ノードに伝播 | **未着手** |
| S6 モード分離+UI | Public/Company/Private分離・モード別起動 | 誤操作でPrivateが漏れない | **未着手** |
| S7 Public限定公開 | 招待制/小規模でフィルタ・失効を実証 | 弁護士レビュー通過 | **未着手** |

現在の最先端(直近で手を付けるべき段階): **S3 P2P化への着手**。S2はゲート(§7フロー通過率を実測)を`bench_pipeline.rs::pipeline_flow_passrate`(代表質問12問の固定セット)で満たし、L2 Agent自己申告/案4トリプル分解/揮発性初期付与(§10.1)/§7フロー判定パイプラインの実装・テスト・脅威モデルレビューを2026-07-17に完了したため「完了」とした(詳細・実測値・残存リスクは下記S2節を参照)。**ただしS3着手前に、脅威モデルレビューで確認された残存リスク(下記S2節「残存リスク(S3着手前に必須)」)への対応が必須**。

---

## 2. 各段階の詳細

### S1 PoC最小ループ — 完了

**裏取り**: `poc/src/{embedder,signer,agent,cache,volatility,main}.rs` の存在確認、`poc/Cargo.toml`(依存: serde/serde_json/sha2/hex/rand/chrono、featureで ed25519/onnx)、`poc/src/tests/{common,test_cache,test_volatility,test_signer,bench_cache}.rs` の存在と `cargo test` 20 passed / 1 ignored(`--features ed25519` でも 20 passed)を確認(2026-07-17時点)。コミット b09d826 でC++からRustへ書き直し済み。

**実行フロー(旧Architecture §13.3 より移植)**:

```text
[単一ノードで完結する最小構成]
1. 質問入力
2. NPUでEmbedding生成 → ローカルVectorDB検索(τ)
3. Hit  → facts取得 → 回答再合成して返却
4. Miss → Agent(1つでよい)へ推論
        → L0/L2判定 → トリプル分解 → volatility付与
        → author署名 → ローカルストアに登録(witnessは自己1件で仮)
5. 再度同義質問 → Hitすることを確認
検証ゴール: 同義質問クラスタが1エントリにHitし、事実型のみ登録される
```

**実装済み**:
- `Embedder`/`Signer`/`Agent` トレイト+ファクトリ関数によるMock/実装差し替え構造(`embedder.rs`, `signer.rs`, `agent.rs`)
- `SemanticCache`(`cache.rs`): brute-force コサイン類似度検索、JSON永続化、ロード時のhash+署名検証(改ざんエントリの読み捨て)
- `entry_id = sha256(signed_payload)` によるID付与とファイル名一致検証
- 揮発性L0ルール+共有可否ANDゲート(`volatility.rs`) — ただしこれはS2スコープの一部先行実装(下記参照)
- `main.rs` の6問デモでHit/Miss/改ざん検知の一連ループが動作確認済み(`docs/PoC_Test_Results.md` §3)
- 単体テスト一式(`poc/src/tests/`、2026-07-17整備完了): `common.rs`(一時ディレクトリヘルパー)+ `test_cache.rs` / `test_volatility.rs` / `test_signer.rs`。`main.rs` に `#[cfg(test)] mod tests` を配線(`#[path]` で `src/tests/` 配下を参照。productionロジックは不変)。カバー範囲: entry_id=sha256(signed_payload)の独立再計算照合、改ざん検知の2経路分離(answer書換=ハッシュ不一致 / author_sig書換=署名検証失敗、設計メモ §4)、HIT/MISS、揮発性分類(volatile/slow)、共有ANDゲートの各単独ブロック+全通過、DummySignerの署名検証ラウンドトリップ・鍵永続化・MACの限界。`cargo test` で 20 passed / 1 ignored、`cargo test --features ed25519` でも 20 passed
- 検索ベンチ `bench_lookup`(`bench_cache.rs`、`#[ignore]`付き)。参考計測値: n=100→約46µs/回、n=1,000→約454µs/回、n=10,000→約5,387µs/回(O(n)線形どおり)。断定的な性能保証ではなく参考計測値であり、`cargo test --release -- --ignored --nocapture bench_lookup` で再計測可能
- `poc/README.md` にテスト実行手順(「テスト」節: `cargo test` とベンチの実行方法)を追加済み

**残作業**: なし(ゲート通過につき「完了」)。
- 注記: witness署名は「自己1件で仮」のまま(単一ノードのため構造上の意図的な省略。S3で実装する意図的縮約として明記済みであり、S1の残作業ではない)

### S2 判定パイプライン — 完了

**裏取り**: `poc/src/{agent,triples,volatility,pipeline,cache}.rs` を読了。L2自己申告(`agent.rs::SelfDeclaration` / `Agent::self_declare`)、案4トリプル分解+述語オントロジー(`triples.rs::decompose` / `PREDICATE_ONTOLOGY` / `predicate_class`)、揮発性初期付与(`volatility.rs::finalize_volatility`、§10.1の4ルールをそのまま実装)、§7フロー判定パイプライン(`pipeline.rs::judge_entry` / `PipelineReport` / `PipelineStage`)の存在と実装内容を確認。`cache.rs::CacheEntry` に `facts: Vec<FactTriple>`(署名対象)と `volatility_confidence` / `volatility_evidence`(署名対象外)が追加され、`signed_payload()` が question+answer+created+volatility(class)+facts をカバーすることを確認(`entry_id = sha256(signed_payload)`・ロード時verifyの不変条件は維持)。テストは `cargo test` で **71 passed / 0 failed / 2 ignored**、`cargo test --features ed25519` でも同じく **71 passed / 2 ignored**(2026-07-17時点)。新規/拡充テストファイル: `test_triples.rs`(21件)・`test_finalize_volatility.rs`(12件)・`test_pipeline.rs`(15件)・`test_cache_facts.rs`(3件)・`bench_pipeline.rs`(1件、`#[ignore]`)。既存 `test_cache.rs`(7件)/`test_volatility.rs`(7件)/`test_signer.rs`(6件)/`bench_cache.rs`(1件、`#[ignore]`)は維持。

**実装済み**:
1. **L0語彙ルール**(S1内で先行実装済み。継続利用): 時間指示語→`volatile`、それ以外→`slow`。共有可否ANDゲート(`share_gate`)の起点段。
2. **L2 Agent自己申告**(Architecture §7.3): `SelfDeclaration { context_independent, factual, volatility }` を `Agent::self_declare` が返す。決定でなく一票として扱い、判定を緩める経路は構造上存在しない(自己申告はvolatilityクラスを動かさず、不一致時にconfidenceを×0.7する安全側のみに反映。§10.1ルール4)。
3. **案4 知識グラフ分解(トリプル分解)**(Architecture §7.3/§10.1): `decompose(answer)` が回答文を決定的ヒューリスティックで `(s, p, o)` へ分解。`PREDICATE_ONTOLOGY`(日英約20述語)が各述語に permanent/slow/volatile を事前付与、`predicate_class()` で照合。
4. **揮発性初期付与ルール(§10.1)の完全実装**: `finalize_volatility(question, answer, decomposition, declared_volatility)` が以下を実装。
   - ルール1: 分解成功かつ全述語がpermanent型 → `permanent`(confidence 0.6)
   - ルール2(最優先): 時間指示語 → 強制 `volatile`(質問・回答の**両方**を走査)
   - ルール3: 分解失敗/未知述語 → デフォルト `slow`
   - ルール4: 自己申告は不一致時にconfidenceを下げる方向のみ(クラスは動かさない)
   - 追加の安全策(脅威レビューMedium-1対応): コピュラ定義述語(種別/is-a)でも目的語が時事シグナル(数値・全角/半角数字・通貨・時点語・年)を含む場合はpermanent昇格を抑止しvolatileへ降格(誤分類非対称性=volatile→permanent誤りの防止)
5. **§7フロー判定パイプライン**: `judge_entry` が L0→L2→トリプル分解→volatility確定を1本で通す。共有可否は「L0保守ゲート AND context_independent AND factual AND 分解成功 AND 全文分解済み AND 全述語オントロジー収録済み AND 非volatile」のANDゲート(S1のL0のみゲートより厳格化する方向のみで、緩める変更ではない)。`PipelineReport` で各段を観測可能にし、`blocked_at` に最初のブロック段(`L0Lexical`/`L2SelfDeclaration`/`TripleDecomposition`/`UnknownPredicate`/`FinalVolatility`)を記録。

**§7フロー通過率の実測(ゲート条件の達成)**

再現コマンド(`poc/` 直下): `cargo test -- --ignored --nocapture pipeline_flow_passrate`(`bench_pipeline.rs`、代表質問12問の固定セット、回帰assert同梱)。2026-07-17時点の実測値:

| 指標 | 値 |
|---|---|
| L0語彙ゲート通過 | 8/12 (66.7%) |
| L2自己申告通過 | 8/12 (66.7%) |
| 案4分解成功 | 8/12 (66.7%) |
| 確定 非volatile | 9/12 (75.0%) |
| **最終共有可(全AND)** | **4/12 (33.3%)** |

§7.4ファネル(最初にブロックした段。12問中の内訳): `L0Lexical`=4 / `L2SelfDeclaration`=1 / `TripleDecomposition`=1 / `UnknownPredicate`=1 / `FinalVolatility`=1 / 全段通過(共有可)=4。

確定volatilityクラス内訳: `permanent`=4 / `slow`=5 / `volatile`=3。

多層ゲートの機能実証: 質問のみを見るL0では非volatileだった「A社の株価は…」型の質問が、回答述語(株価等)由来のvolatileとして確定段で捕捉され共有除外される。未知述語(「犬の色は茶色です」の述語「色」)はslowのままローカル保持可だが、共有はUnknownPredicate段でブロックされる(S2では共有可を述語オントロジー収録済みのallowlistに限定する保守的変更)。

上記により、ゲート条件「§7フロー通過率を実測」を達成したため「完了」とする。

**残存リスク(S3着手前に必須。脅威モデルレビューで確認)**:

a. embeddingが署名対象外かつロード時に再計算されない → question(署名済み)から再計算する対応が必要。
b. `shareable` が署名対象外、かつL2自己申告(context_independent/factual)がエントリに保存されないため、受信ノードが `judge_entry` の判定結果を再現できない → S3では**受信側で共有可否を再判定(shareableを信頼せず再導出)することを必須要件化**する。
c. provenance(agent)が未署名(PoCの意図的縮約) → Architecture §6完全準拠のため署名対象化する。

**残存リスク(S3で受信answerを再分解する設計にする場合の追加要件)**: `decompose` の準二次スキャンに対し、敵対的長文入力の入力長・反復回数上限(DoS対策)を設ける。

**S2として許容する安全側の残存(誤って過剰ブロックする方向=汚染ではない。将来課題)**:
- L0語彙の部分文字列一致による誤爆(英語 "now"⊂"know"、日本語「いま」⊂「います」)
- `contains_standalone_number` のMP3型(末尾数字)誤検知
- 漢数字・単位なし綴りの時事値がガードを素通りする

いずれも「疑わしきは共有しない」を過剰に効かせる方向であり毒混入方向ではないが、実埋め込み/多言語対応時に共有率を毀損しうるため、語彙・数値境界の精緻化を将来課題とする(下記§3の「述語オントロジーの初期構築方法と多言語対応」「共有用τの実測チューニング」と関連)。

### S3 P2P化 — 未着手

DHT分散・witness署名・複数版併存はいずれも `poc/` に該当コードなし(`witness_sigs`は`CacheEntry`に存在せず、`PoC_Design_Notes.md`のデータモデル対応表にも記載なし)。トップレベル `src/core/`, `src/ui/`(CLAUDE.md記載の将来レイアウト)も未作成(2026-07-17時点でリポジトリに存在しないことを確認)。

### S4 評判・独立検証 — 未着手

3層評判・スラッシング・層3抜き打ち再推論に対応するコードなし。`trust`フィールド(Architecture §6の`independent_agreement`等)は`poc/src/cache.rs`の`CacheEntry`に存在しない。

### S5 法的機構 — 未着手

regurgitationフィルタ・revocationフラッディング・出所記録(`provenance`)は`poc/`に対応コードなし。`CacheEntry`に`provenance`フィールドは存在しない。

### S6 モード分離+UI — 未着手

Public/Company/Privateのモード分離起動、UI層(C# Blazor、CLAUDE.md記載の`src/ui/`)はまだ着手されていない(ディレクトリ自体が存在しない)。

### S7 Public限定公開 — 未着手

前提となるS3〜S6が未着手のため、当然ながら未着手。§11の法的レビュー(専門弁護士レビュー)もまだ実施段階にない。

---

## 3. 未解決事項(旧 Architecture §13.1 より移植)

| 未解決事項 | 関連する主な段階 |
|---|---|
| Embeddingモデルの最終選定(MPNet級を推奨だが未確定)と共有用τの実測チューニング | S2(実測)/S3以降(実運用選定) |
| 述語オントロジー(揮発性クラス事前付与)の初期構築方法と多言語対応 **[補完: 元メモに具体設計なし。案4運用の前提として要設計]** | S2(案4トリプル分解の前提) |
| 受信ノードでの共有可否再判定(`shareable`を信頼せず再導出)を必須要件化 **[脅威レビュー(2026-07-17)。詳細はS2節「残存リスク(S3着手前に必須)」参照]** | S3 |
| embedding の署名対象化・ロード時再計算(question由来) **[脅威レビュー(2026-07-17)。詳細は同上]** | S3 |
| provenance(agent)の署名対象化(Architecture §6完全準拠) **[脅威レビュー(2026-07-17)。詳細は同上]** | S3 |
| `decompose` の入力長・反復回数上限(DoS対策。受信answerを再分解する設計にする場合) **[脅威レビュー(2026-07-17)。詳細は同上]** | S3 |
| 局所EigenTrustの近傍サイズ・伝播ホップ数・スラッシング係数の具体パラメータ **[補完]** | S4 |
| regurgitationフィルタの参照コーパスをどう用意するか(著作物データベース非保有問題)**[補完]** | S5 |
| 誕生証明PoWの難易度調整(ネットワーク規模に応じた動的調整)**[補完]** | S4 |
| DHT実装の選定(既存Kademlia系流用可否)**[補完]** | S3 |

---

## 4. 更新履歴

- 2026-07-17: 新規作成。Architecture.md §13.1/§13.2/§13.3 を移植し、poc/ の実ソースを確認した上で各段階のステータスを付与。
- 2026-07-17: S1テスト整備完了(`poc/src/tests/` 追加、`cargo test` 20 passed / 1 ignored、`poc/README.md` テスト節追加)を受け、S1ステータスを「一部完了」→「完了」に更新。「現在の最先端」をS2着手優先の記述に変更。
- 2026-07-17: S2判定パイプライン(L2 Agent自己申告/案4トリプル分解/揮発性初期付与§10.1/§7フローパイプライン)を実装・テスト・脅威モデルレビューし、§7フロー通過率の実測(ゲート条件)を`bench_pipeline.rs::pipeline_flow_passrate`で達成したため、S2ステータスを「一部着手(L0のみ)」→「完了」に更新。`cargo test` 71 passed / 2 ignored(両feature)。脅威モデルレビューで確認されたS3着手前必須の残存リスク3件をS2節・§3未解決事項に記録。「現在の最先端」をS3着手優先の記述に変更。
