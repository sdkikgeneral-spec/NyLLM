# 分散セマンティックキャッシュ — 実装ロードマップ

> 出典: 本ファイルは [Architecture.md](./Architecture.md) 旧§13.2「実装ステップ」+ 旧§13.1「未解決事項」+ 旧§13.3「PoC最小ループ(S1詳細)」を抽出・統合したもの。
> 二重管理回避のため、**実装ステップの内容(段階/ゲート)と進捗ステータスの一次情報はこのファイルのみ**とする。Architecture.md 側は参照リンクのみを残す。
> 前提ドキュメント: [Architecture.md](./Architecture.md) / [信頼性設計メモ.md](./信頼性設計メモ.md) / [PoC_Design_Notes.md](./PoC_Design_Notes.md) / [PoC_Test_Results.md](./PoC_Test_Results.md)
> 作成日: 2026-07-17
> ステータス凡例: **未着手** / **一部着手**(範囲の一部のみ着手・段階の目標としては未達) / **一部完了**(主要機能は動くが残作業あり) / **完了**(ゲート通過)
> ステータスは `poc/`(および将来の `src/core`, `src/ui`)の実ソースを確認して記載している。裏取り方法は各段階の節を参照。

---

## 0. 主戦場の段階展開(Company Phase1 → Public Phase2)【オーナー採用 2026-07-17】

> 出典: [設計レビュー_2026-07.md](./設計レビュー_2026-07.md) §4.7(採用決定の経緯・8軸比較)/ [信頼性設計メモ.md](./信頼性設計メモ.md) §9(一次設計判断)/ [Architecture.md](./Architecture.md) §9([補完]反映)。**本節は既存のS1〜S7の段階定義・ゲート・進捗ステータス(下記§1・§2)を変更しない。段階展開はS1〜S7の上に重なる位置づけであり、S1〜S7自体の意味・ゲート条件は不変。**

### 更新シーケンス

- **[A] 戦略ゲート** — 決着(Company先行段階展開 / 幻覚パリティは概念Phase1・強制Phase2 / アンカー・ステークはPhase2繰り延べ+空スロット確保)。残作業はdocs反映のみで、本反映(2026-07-17)により完了。
- **[B] S2.5 エントリ形式の確定** — **実装完了**(2026-07-17)。確定設計は [S2.5_エントリ形式設計.md](./S2.5_エントリ形式設計.md) を参照(詳細な経緯は下記§5。実装後の既知の妥協点・残存事項は同ノート§13)。実装対象は `poc/src/cache.rs`(immutable_core/mutable_state分離・serde非依存のcore_bytes正準化・entry_id/question_key算出・§6のload/verify 10手順)・`poc/src/signer.rs`(`Signer` trait を `sign_bytes`/`verify` へ変更、`DummySigner` を HMAC-SHA256 化)・`poc/src/signer/ed25519_signer.rs`(core_bytesへの署名に追随)・`poc/src/main.rs`(新形式での通しデモ)。対応テストは `test_cache.rs`/`test_cache_facts.rs`/`test_signer.rs` を更新し、`cargo build`(既定/`--features ed25519`)通過、`cargo test` 82 passed(`--features ed25519` でも83 passed)、`cargo run` の6問デモ+改ざん検知が新形式で動作することを確認。脅威モデルレビューも実施済み(Critical/High の新規ブロッカーなし。残存事項は下記§5行およびS2.5ノート§13参照)。不変コアはPublicまで見据えてフル確定済みで、可変状態は空スロット(`trust`/`witness_sigs`/`anchor_proof`/`stake`)を型で宣言済み(Phase2は中身を埋めるだけで済む)。`poc/` 上・単一ノードのまま(P2P化そのものは引き続きS3)。
- **[C] `src/core`(Rust)への昇格 = Company Phase1本体** — 背骨(trait 3本/エントリ形式の不変コア/triples/volatility/共有ゲート/検索)を移植。**S3相当を"社内版"に縮約**(多ノード共有はやるが、witness/アンカーは共通時計で代替)。具体設計は [S3_Company_Phase1_社内多ノード共有設計.md](./S3_Company_Phase1_社内多ノード共有設計.md) として確定済み。**実装完了**(2026-07-18): 背骨の`src/core`移植+S3縮約範囲(レジストリ発見+ノード間直接配送+Company/Private起動分離)を`src/core`+別クレート`registry`へ実装し、`cargo test --workspace`が両ビルド(default/`--features ed25519`)で全緑、脅威モデルレビュー承認済み(詳細は下記§1・§2「S3 P2P化」節)。**S4評判の大半・S5法務フル・S7はPhase2に繰り延べ。S6モード分離(Company/Private起動分離)はPhase1で着手可**(実際にS3実装内で先行実装済み。UI層は未着手 → §2 S6節)。`poc/` はS1/S2の参照実装として凍結・保存済み。
- **[移行ゲート] Phase1 → Phase2 条件**: ①不変コア安定、②共有ゲート通過率の社内実測、③Tier-L幻覚パリティの非劣化実測、④空スロット設計(`witness_sigs`/`anchor_proof`/`stake`/`trust`)が机上で閉じる、⑤弁護士レビュー着手。
- **[D] Public Phase2** — 空スロットを埋める(作り直しでなく追加): witness+アンカー二層 / 評判ステーク+外部照合裁定 / Tier-H強制 / revocation+regurgitation / PoW ID / 段階公開(S7)。

### 既存ステージとの対応(定義・ゲート自体は不変)

| 段階 | 段階展開上の位置づけ |
|---|---|
| S1 / S2 | 完了済み。Company Phase1の背骨として引き続き有効(そのまま[C]へ移植) |
| S3 P2P化 | Company Phase1では"社内版"に縮約(多ノード共有はやるがwitness署名/アンカーは共通時計で代替)。フルP2P(DHT・witness循環対策込み)はPublic Phase2で再拡張。具体設計は [S3_Company_Phase1_社内多ノード共有設計.md](./S3_Company_Phase1_社内多ノード共有設計.md) を参照 |
| S4 評判・独立検証 | 3層評判・スラッシングの大半はPhase2繰り延べ。層1(トリプル一致率によるエントリ内在信頼度)相当のみPhase1で先行しうる(具体設計は [S4_Company_Phase1_層1内在信頼度先行設計.md](./S4_Company_Phase1_層1内在信頼度先行設計.md) を参照) |
| S5 法的機構 | regurgitationフィルタ・revocationフラッディングのフル実装はPhase2繰り延べ(社内配布のみのCompany Phase1では法的圧力が相対的に低いため) |
| S6 モード分離+UI | Company/Private起動分離はPhase1で着手可。Public起動分離の本格運用はPhase2 |
| S7 Public限定公開 | 定義上Phase2専属(移行ゲート通過後) |

---

## 1. 実装ステップ全体表 (S1〜S7)

| 段階 | 内容 | ゲート(通過条件) | 現状ステータス |
|---|---|---|---|
| S1 PoC最小ループ | Embedding検索 → ミス時Agent呼び出し → 署名付きキャッシュ登録(単一ノード) | Hit/Missが動く | **完了** |
| S2 判定パイプライン | L0/L2ゲート+案4トリプル分解+揮発性初期付与 | §7フロー通過率を実測 | **完了** |
| S3 P2P化 | DHT分散・witness署名・複数版併存 | 2ノード以上で共有 | **完了(Company Phase1縮約範囲)**(注記: フルP2P〔DHT・witness〕は§0対応表どおりPublic Phase2で再拡張) |
| S4 評判・独立検証 | 3層評判・スラッシング・層3抜き打ち | 毒注入テストに耐える | **未着手** |
| S5 法的機構 | regurgitationフィルタ・revocationフラッディング・出所記録 | 失効が全ノードに伝播 | **未着手** |
| S6 モード分離+UI | Public/Company/Private分離・モード別起動 | 誤操作でPrivateが漏れない | **一部着手**(Company/Private起動分離のみS3実装内で先行実装。UI・Public分離・ゲート判定は未達 → §2 S6節) |
| S7 Public限定公開 | 招待制/小規模でフィルタ・失効を実証 | 弁護士レビュー通過 | **未着手** |

現在の最先端(直近で手を付けるべき段階): **S4 評判・独立検証(層1内在信頼度の先行実装)**。S2はゲート(§7フロー通過率を実測)を`bench_pipeline.rs::pipeline_flow_passrate`(代表質問12問の固定セット)で満たし、L2 Agent自己申告/案4トリプル分解/揮発性初期付与(§10.1)/§7フロー判定パイプラインの実装・テスト・脅威モデルレビューを2026-07-17に完了したため「完了」とした(詳細・実測値・残存リスクは下記S2節を参照)。**S3着手前に必須とされていた残存リスク(embedding署名対象化・受信側shareable再導出・provenance署名対象化)はS2.5(エントリ形式再設計)の実装完了(2026-07-17。上記§0[B]参照)により解消済み**。S2.5の脅威モデルレビューでCritical/Highの新規ブロッカーは確認されていないが、情報として記録された残存事項(reload時再導出の非単調性・initial_tier既定値・版フラッディング・state.jsonのノードローカル制約)は [S2.5_エントリ形式設計.md](./S2.5_エントリ形式設計.md) §13 にS3持ち越しとして記録済み。**S3(Company Phase1縮約範囲)は`src/core`+ 別クレート`registry`への実装・全テスト緑・脅威モデルレビュー承認を2026-07-18に完了したため「完了」とした(詳細は下記§2「S3 P2P化」節を参照)**。S4は層1(トリプル一致率によるエントリ内在信頼度)先行設計ノート([S4_Company_Phase1_層1内在信頼度先行設計.md](./S4_Company_Phase1_層1内在信頼度先行設計.md))が確定済みだが実装は未着手のままであり、S3実装確定後の依存前提差分再検証を経て着手する。

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
- `entry_id = sha256(signed_payload)` によるID付与とファイル名一致検証(S1時点の形式。**2026-07-17のS2.5実装により `entry_id = hex(sha256(core_bytes))` へ置き換え済み**。詳細は上記§0[B]・[S2.5_エントリ形式設計.md](./S2.5_エントリ形式設計.md) §4)
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

**上記a〜cの解消(2026-07-17、S2.5実装完了)**: a(embeddingは非保存とし、ロード時に`local_embedder`で再計算=改良案C)/b(§6手順8で`judge_entry`をロード時に再実行し`shareable`/`tier_operative`/`volatility_class_operative`を必ず再導出、送信者側の値は不信任)/c(`provenance.agent`を`immutable_core`に含めて署名対象化)は、いずれも [S2.5_エントリ形式設計.md](./S2.5_エントリ形式設計.md) の実装(`poc/src/cache.rs`)で解消済み。詳細は同ノート§1・§2・§6、実装完了の裏取りは上記§0[B]・下記§5のS2.5行を参照。

**残存リスク(S3で受信answerを再分解する設計にする場合の追加要件)**: `decompose` の準二次スキャンに対し、敵対的長文入力の入力長・反復回数上限(DoS対策)を設ける。

**S2として許容する安全側の残存(誤って過剰ブロックする方向=汚染ではない。将来課題)**:
- L0語彙の部分文字列一致による誤爆(英語 "now"⊂"know"、日本語「いま」⊂「います」)
- `contains_standalone_number` のMP3型(末尾数字)誤検知
- 漢数字・単位なし綴りの時事値がガードを素通りする

いずれも「疑わしきは共有しない」を過剰に効かせる方向であり毒混入方向ではないが、実埋め込み/多言語対応時に共有率を毀損しうるため、語彙・数値境界の精緻化を将来課題とする(下記§3の「述語オントロジーの初期構築方法と多言語対応」「共有用τの実測チューニング」と関連)。

### S3 P2P化 — 完了(Company Phase1縮約範囲)

**裏取り**: `src/core/`(`node.rs`/`transport.rs`/`wire.rs`/`sync.rs`/`registry_client.rs`/`daemon.rs`/`policy.rs`/`main.rs`)+ 別クレート `registry` の存在・実装内容を確認。`cargo test --workspace` が default / `--features ed25519` の両ビルドで **全緑**(**67 passed / 0 failed / 2 ignored**。実装コミット時点は66 passedで、その後の軽微なテスト追補により67で確定・両ビルド実測済み。2026-07-18)。S3ゲート判定テスト `s3_gate_two_or_more_nodes_share`(`src/tests/test_sync.rs`。3ノードで shareable エントリが全ノードに複製されることを assert)が両ビルドで緑であることを確認。脅威モデルレビュー(2026-07-18)実施済み・判定=承認(修正必須なし、production不変条件はすべて維持)。

**実装内容の要約**(具体設計は [S3_Company_Phase1_社内多ノード共有設計.md](./S3_Company_Phase1_社内多ノード共有設計.md) を参照):

- レジストリ発見(参加/離脱・ピア一覧・CA公開鍵/CRL配布のみを担う軽量`registry`クレート)+ ノード間直接HTTP配送(Announce/Request/Transfer/Digestの`nyllm-wire/v1`)。
- 受信側検証手順1〜10(エンベロープ解析 → 組織PKI検証〔`node_cert`のCA署名+CRL未失効〕→ ハッシュ照合 → 署名検証 → コアパース → `question_key`再計算 → embedding再計算 → `judge_entry`再実行による`shareable`/`tier_operative`/`volatility_class_operative`の再導出 → 重複排除・冪等マージ)。送信者申告の`mutable_state`は一切信頼しない。
- 冪等マージ・複数版併存(`entry_id`単位のgrow-only集合。同一`question_key`×異`entry_id`は別版として併存)・anti-entropy(Digest定期交換による取りこぼし補償)。
- `--mode company` / `--mode private` の起動時モード分離(別`store_dir`、Privateは配送層〔transport/sync/registry_client〕を一切インスタンス化しない、レジストリ未登録で`/registry/peers`にも現れない)。
- Phase2への非破壊性を担保するポリシー差し替えフック4点(cert検証/時刻検証/失効フィルタ/発見層。`policy.rs`)。
- 脅威レビュー由来で先行実装した対処: **H-1**(CRL失効著者の既取込エントリを検索・供出・Digestの各パスで遡及除外。cert表に依存せず`author_pub`から`node_id`を直接再計算してCRL照合)、**M-1**(CA公開鍵のピン留め+TOFU。初回の非空CA供給で固定し、以後レジストリが偽CAを配布してもピンは上書きされない)、**M-2**(shareable単調性保護。reload時`shareable`を「再導出値 AND ディスク値」で合成し、非単調な反転〔false→true〕を防ぐ。回帰テスト`test_monotonic_shareable.rs`)。

**ゲート達成根拠**: ゲート条件「2ノード以上で共有」は`s3_gate_two_or_more_nodes_share`(3ノード構成でshareableエントリが全ノードに複製されることをassert)が default / `--features ed25519` の両ビルドで緑であることに加え、registry実HTTP統合スモーク(join/peers/ca/refresh_once、CAピン留め/TOFU、privateノードが`/registry/peers`に現れないこと)を含む全テストが緑であることをもって達成とする。

**残課題**(Phase2送り・注記):
- CRLはレジストリから無署名・平文HTTPで配布されるため、レジストリcompromise時にCRL検閲(正規ノードの失効注入)が成立しうる(Phase1既知制約)。Phase2でCA署名付きCRLにより対処する。
- `NodeConfig`のTTL(volatile=1時間、slow=30日)はPhase1暫定値としてコード側で設定されたものであり、確定チューニングは未対応のまま(§11-7とも関連)。
- `decompose`の入力長・反復回数上限(DoS対策)は依然未対応のまま(下記§3の該当行を参照。今回の実装範囲では対応していない)。
- エントリ単位失効(`RevocationPolicy`)は受信側フィルタ(検索除外・anti-entropyプル前)のみに配線されており、供出/Digest列挙のソース側は著者CRLのみを参照する非対称がある。Phase2でエントリ失効の中身を実装する際、ソース側フィルタの要否を再評価すること。

上記により、ゲート条件「2ノード以上で共有」を達成したため、本表(§1)のS3ステータスを「未着手」→「**完了(Company Phase1縮約範囲)**」に更新する。フルP2P(DHT・witness循環対策込み)はPublic Phase2で再拡張する(§0対応表どおり)。

### S4 評判・独立検証 — 未着手

3層評判・スラッシング・層3抜き打ち再推論に対応するコードなし。**注記(2026-07-17、S2.5実装後)**: `trust`フィールド(Architecture §6の`independent_agreement`等に相当)はS2.5により`MutableState`の**空スロット(Phase2予約。常に`None`)**としては存在するが、中身(評判集計ロジック)は未実装のままである([S2.5_エントリ形式設計.md](./S2.5_エントリ形式設計.md) §2)。

**注記(2026-07-17、S4層1先行設計ノート追加)**: 3層評判のうち**層1(トリプル一致率によるエントリ内在信頼度)のみを対象とするCompany Phase1先行設計ノート**を [S4_Company_Phase1_層1内在信頼度先行設計.md](./S4_Company_Phase1_層1内在信頼度先行設計.md) として追加した(層2/層3はPublic Phase2の範囲外のまま)。同ノートは「先行設計(docs)・実装は別段階(未着手)」であり、実装はS3(Company Phase1多ノード共有)の実装確定後に依存前提を差分再検証してから着手される。**追記(2026-07-18)**: S3実装が確定したため(→上記S3節)、同ノート§2「S3依存前提(仮リスト)」の差分再検証が実施可能になった(差分再検証自体は未実施。S4着手時の最初の作業)。**本表(§1)のS4ステータス「未着手」は変更しない。**

### S5 法的機構 — 未着手

regurgitationフィルタ・revocationフラッディングに対応するコードなし。**注記(2026-07-17、S2.5実装後)**: 出所記録は`immutable_core.provenance`(`agent`/`model`/`embedder_model_id`)としてS2.5で署名対象化・実装済みだが、これはPoCの生成主体メタデータの記録であり、S5が指すregurgitationフィルタ・revocationフラッディング・失効伝播の機構自体は引き続き未着手である([S2.5_エントリ形式設計.md](./S2.5_エントリ形式設計.md) §1)。

### S6 モード分離+UI — 一部着手

**Company/Private の起動時モード分離のみ、S3実装(2026-07-18)の一部として先行実装済み**(§0[C]「S6モード分離はPhase1で着手可」の範囲): `src/core/main.rs` の `--mode company|private` 起動分岐(別`store_dir`=`company_store`/`private_store`、Privateは配送層〔transport/sync/registry_client〕を一切インスタンス化しない)、および回帰テスト `test_registry_integration.rs::private_node_never_appears_in_registry`(privateノードがレジストリ`/registry/peers`に現れないこと)。詳細は上記S3節・[S3_Company_Phase1_社内多ノード共有設計.md](./S3_Company_Phase1_社内多ノード共有設計.md) §6。

一方、**Publicモードの分離(定義上Phase2)と、UI層(C# Blazor、CLAUDE.md記載の`src/ui/`)は未着手**(`src/ui/` ディレクトリ自体が存在しない)。ゲート条件「誤操作でPrivateが漏れない」の総合判定(UI操作経路を含む)も未実施のため、段階の目標としては未達=「一部着手」とする(ゲート定義自体は不変)。

### S7 Public限定公開 — 未着手

前提のうちS3はCompany Phase1縮約範囲で完了したが(フルP2PはPhase2再拡張待ち)、S4〜S6が未完了のため未着手。定義上Phase2専属(§0対応表)。§11の法的レビュー(専門弁護士レビュー)もまだ実施段階にない。

---

## 3. 未解決事項(旧 Architecture §13.1 より移植)

| 未解決事項 | 関連する主な段階 |
|---|---|
| Embeddingモデルの最終選定(MPNet級を推奨だが未確定)と共有用τの実測チューニング | S2(実測)/S3以降(実運用選定) |
| 述語オントロジー(揮発性クラス事前付与)の初期構築方法と多言語対応 **[補完: 元メモに具体設計なし。案4運用の前提として要設計]** | S2(案4トリプル分解の前提) |
| 受信ノードでの共有可否再判定(`shareable`を信頼せず再導出)を必須要件化 **[脅威レビュー(2026-07-17)。詳細はS2節「残存リスク(S3着手前に必須)」参照]** — **解消済み(2026-07-17、S2.5実装。§6手順8で`judge_entry`をロード時に再実行)** | S3 |
| embedding の署名対象化・ロード時再計算(question由来) **[脅威レビュー(2026-07-17)。詳細は同上]** — **解消済み(2026-07-17、S2.5実装。embeddingは非保存・ロード時に再計算)** | S3 |
| provenance(agent)の署名対象化(Architecture §6完全準拠) **[脅威レビュー(2026-07-17)。詳細は同上]** — **解消済み(2026-07-17、S2.5実装。`provenance.agent`を`immutable_core`に含め署名対象化)** | S3 |
| `decompose` の入力長・反復回数上限(DoS対策。受信answerを再分解する設計にする場合) **[脅威レビュー(2026-07-17)。詳細は同上]** | S3(S2.5の対象外。**S3実装(2026-07-18)でも未対応のまま**=受信側は`judge_entry`再実行で実際に再分解するため引き続き有効な課題。Phase2送り。→§2 S3節「残課題」) |
| 局所EigenTrustの近傍サイズ・伝播ホップ数・スラッシング係数の具体パラメータ **[補完]** | S4 |
| regurgitationフィルタの参照コーパスをどう用意するか(著作物データベース非保有問題)**[補完]** | S5 |
| 誕生証明PoWの難易度調整(ネットワーク規模に応じた動的調整)**[補完]** | S4 |
| DHT実装の選定(既存Kademlia系流用可否)**[補完]** | S3(Company Phase1縮約ではDHT不採用=レジストリ発見で代替のため未着手のまま。フルP2P再拡張時=Public Phase2に持ち越し。→§0対応表) |

---

## 4. 更新履歴

- 2026-07-17: 新規作成。Architecture.md §13.1/§13.2/§13.3 を移植し、poc/ の実ソースを確認した上で各段階のステータスを付与。
- 2026-07-17: S1テスト整備完了(`poc/src/tests/` 追加、`cargo test` 20 passed / 1 ignored、`poc/README.md` テスト節追加)を受け、S1ステータスを「一部完了」→「完了」に更新。「現在の最先端」をS2着手優先の記述に変更。
- 2026-07-17: S2判定パイプライン(L2 Agent自己申告/案4トリプル分解/揮発性初期付与§10.1/§7フローパイプライン)を実装・テスト・脅威モデルレビューし、§7フロー通過率の実測(ゲート条件)を`bench_pipeline.rs::pipeline_flow_passrate`で達成したため、S2ステータスを「一部着手(L0のみ)」→「完了」に更新。`cargo test` 71 passed / 2 ignored(両feature)。脅威モデルレビューで確認されたS3着手前必須の残存リスク3件をS2節・§3未解決事項に記録。「現在の最先端」をS3着手優先の記述に変更。
- 2026-07: `docs/設計レビュー_2026-07.md`(fableによるレビュー記録)へのactionable索引を§5として追加(段階/ゲートの定義・ステータス列は変更なし)。
- 2026-07: `docs/設計レビュー_2026-07.md` §4(対話ログ:オーナーの4直感と統治目標の再定義)追加に伴い、§5関連レビュー記録の索引を拡充(未採用の検討事項3件を追加、S2.5行に§4.2を追記)。段階定義・ゲート・進捗ステータス(§1・§2)は不変。
- 2026-07-17: オーナーが戦略ゲート[A]「主戦場=Company Phase1先行 → Public Phase2後続の段階展開」を採用決定(`設計レビュー_2026-07.md` §4.7)。本ファイルに§0(段階展開と更新シーケンス[A]〜[D]・既存S1〜S7ステージとの対応)を新設し、§5でS2.5を「戦略ゲート決着・着手可能」に更新(移行ゲート5条件を記載)、§5に「主戦場の段階展開(採用決定)」行を追加。`Architecture.md` §9・§2.2に[補完]で本体反映、`信頼性設計メモ.md` §9に一次記載を新設(旧§9「未解決/次に詰めること」は§10へ繰り下げ)。段階展開はS1〜S7自体の定義・ゲート・進捗ステータス(§1・§2)を変更しない。
- 2026-07-17: S2.5(エントリ形式再設計)の確定設計ノート `docs/S2.5_エントリ形式設計.md` を新規作成(`設計レビュー_2026-07.md` §2/§4の改良案A〜Eを具体化)。本ファイル §0[B] と §5 のS2.5行を本ノートへのリンク付き・「設計完了・実装待ち(承認後 poc-core-dev)」に更新。実装(`poc/src/cache.rs`/`signer.rs`)は未着手のまま。段階定義・ゲート・進捗ステータス(§1・§2)は不変。
- 2026-07-17: S2.5(エントリ形式再設計)を実装(`poc/src/cache.rs`/`signer.rs`/`signer/ed25519_signer.rs`/`main.rs`、対応テスト`test_cache.rs`/`test_cache_facts.rs`/`test_signer.rs`)。`cargo build`(既定/`--features ed25519`)通過、`cargo test` 82 passed(`--features ed25519` でも83 passed)、`cargo run` の6問デモ+改ざん検知が新形式で動作、脅威モデルレビュー実施済み(Critical/Highブロッカーなし)を確認し、本ファイル §0[B] と §5 のS2.5行を「実装完了」に更新。これによりS2節「残存リスク(S3着手前に必須)」a〜c(embedding署名対象化・受信側shareable再導出・provenance署名対象化)および§3未解決事項の対応する3行を解消済みとして注記(§1・§2のS1/S2ゲート判定自体には影響なし)。脅威モデルレビューで情報として記録されたS3持ち越し事項(reload非単調性・initial_tier既定値・版フラッディング・state.jsonノードローカル制約)を `docs/S2.5_エントリ形式設計.md` §13に新設して記録。段階定義・ゲート・進捗ステータス(§1・§2の表自体)は不変。
- 2026-07-17: S3(Company Phase1)社内多ノード共有設計ノート `docs/S3_Company_Phase1_社内多ノード共有設計.md` を新規作成(中央レジストリ発見+ノード間直接配送・S2.5 load/verifyの受信側再実行・組織PKIによるシビル不在の簡約・Company/Private起動分離・Public Phase2への非破壊性・S2.5 §13残課題との対応を確定)。本ファイル §0[C]・§1対応表のS3行・§2「S3 P2P化」節・§5に本ノートへのリンクを追加。S3自体の段階定義・ゲート・進捗ステータス(§1・§2の表自体、S3=「未着手」)は不変。実装(`src/core`移植・新規モジュール)は本ノート承認後の別段階として未着手のまま。
- 2026-07-17: S4層1先行設計ノート `docs/S4_Company_Phase1_層1内在信頼度先行設計.md` を新規作成(3層評判のうち層1〔独立生成回答のトリプル一致率〕のみを対象とするCompany Phase1先行設計。層2/層3はPublic Phase2の範囲外のまま、S3 §3手順9への相乗り・S3依存前提の仮リスト化・作用範囲を検索ランキング補助+UI表示の助言のみに限定・Public Phase2への非破壊性を規定)。本ファイル §0対応表のS4行・§2「S4 評判・独立検証」節・§5に本ノートへのリンクを追加。**S4ゲート判定(§1、「未着手」)は不変**。実装は本ノート承認後かつS3実装確定後の別段階として未着手のまま。
- 2026-07-18: S3(Company Phase1)を実装完了。`src/core/`(`node.rs`/`transport.rs`/`wire.rs`/`sync.rs`/`registry_client.rs`/`daemon.rs`/`policy.rs`/`main.rs`)+ 別クレート `registry` に、[S3_Company_Phase1_社内多ノード共有設計.md](./S3_Company_Phase1_社内多ノード共有設計.md) §3〜§9をほぼ完全に実装。`cargo test --workspace` が default / `--features ed25519` の両ビルドで全緑(66 passed / 0 failed / 2 ignored)、S3ゲート判定テスト`s3_gate_two_or_more_nodes_share`(3ノードでshareableエントリが全ノードに複製されることをassert)が両ビルドで緑であることを確認。脅威モデルレビュー(2026-07-18)実施済み・判定=承認(修正必須なし、production不変条件はすべて維持)。レビュー由来で先行実装したH-1(CRL失効著者の遡及除外)/M-1(CA公開鍵ピン留め+TOFU)/M-2(shareable単調性保護)、Phase1既知制約(CRL無署名配布によるレジストリ侵害時の検閲リスク)、Phase2申し送り(エントリ単位revocationのソース側フィルタ非対称・TTL確定チューニング)を`S3_Company_Phase1_社内多ノード共有設計.md`のステータス欄・§7・§8・§10に反映し、本ファイル §1のS3ステータスを「未着手」→「完了(Company Phase1縮約範囲)」に、§2「S3 P2P化」節を実装完了の内容に更新。フルP2P(DHT・witness)はPublic Phase2で再拡張(§0対応表・変更なし)。§3未解決事項の「DHT実装の選定」行・「`decompose`の入力長・反復回数上限」行はPhase2/未対応のまま維持(今回のスコープ外)。S1/S2/S4〜S7の段階定義・ゲート・進捗ステータスは変更なし。
- 2026-07-18: S3完了反映後の整合レビュー(fable)。`src/core`/`src/registry`/`src/tests` の実ソース裏取りに基づき以下を修正: ①S3節裏取りのテスト数を確定値 **67 passed / 0 failed / 2 ignored**(両ビルド実測)に更新(実装コミット時点66→テスト追補で67)、②§0[C]を「実装完了(2026-07-18)」に更新(S3実装完了との食い違い解消)、③S6ステータスを「未着手」→「**一部着手**」に更新(Company/Private起動分離+`private_node_never_appears_in_registry`回帰テストがS3実装内で先行実装済みという事実と旧記述「モード分離起動は未着手」の矛盾を解消。UI・Public分離・ゲート総合判定は未達のまま)、④S7節の前提記述を現状(S3完了・S4〜S6未完了)に更新、⑤§3の`decompose`行・DHT行に現状注記を追加、⑥§1「現在の最先端」の参照誤記(「下記S2節」→「下記§2節」)とS4節の誤字(ノード→ノート)を修正、⑦S4節にS3実装確定による差分再検証が実施可能になった旨を追記(S4「未着手」は不変)。ゲート定義・S1〜S5/S7のゲート判定は変更なし。あわせて [S4_Company_Phase1_層1内在信頼度先行設計.md](./S4_Company_Phase1_層1内在信頼度先行設計.md) §2冒頭の「S3は実装未着手」記述に同趣旨の追記を実施。

---

## 5. 関連レビュー記録

> 以下はレビュー指摘・検討事項への索引であり、段階定義・ゲート・進捗ステータス(§1・§2)には一切影響しない(§0の段階展開もS1〜S7自体の定義・ゲートは変更しない)。**1行目(主戦場の段階展開)はオーナーが戦略として採用決定し、本体反映済み**(→ §0、[Architecture.md](./Architecture.md) §9 [補完]、[信頼性設計メモ.md](./信頼性設計メモ.md) §9)。それ以外の行(個別機構・時刻アンカー/毒対策/統治目標の強制機構)は引き続き未採用の検討事項であり、正の設計docへの本体反映はPublic Phase2での判断待ち。

| 参照元 | 内容 | 索引先 |
|---|---|---|
| 主戦場の段階展開(採用決定) | Public vs Company の8軸比較で全軸Company優位。戦略ゲート[A]の決着としてCompany Phase1先行 → Public Phase2後続の段階展開を採用。Architecture.md §9([補完])・信頼性設計メモ.md §9・本ファイル§0に本体反映済み | [設計レビュー_2026-07.md](./設計レビュー_2026-07.md) §4.7 |
| S3 着手前提 | S3(P2P化)着手前に閉じるべきセキュリティ トップ5(witness循環/受信側再判定/DHT eclipse耐性/共通原因故障/revocation権限モデル)。Company Phase1では"社内版"に縮約(→ §0対応表) | [設計レビュー_2026-07.md](./設計レビュー_2026-07.md) §3 |
| S2.5(実装完了) エントリ形式再設計 | entry_id/署名スキーマの改良案A〜E(署名対象バイト列化・不変コア/可変状態分離・embedding非保存・ID二層化・形式衛生)+アンカー参照フィールド(`timestamp_proof`)の統合を含め、不変コアはPublicまで見据えフル確定・可変状態は空スロット(`witness_sigs`/`anchor_proof`/`stake`/`trust`)を型で宣言。移行ゲート(Phase1→Phase2)条件: ①不変コア安定/②ゲート通過率の社内実測/③Tier-L非劣化実測/④空スロット設計が机上で閉じる/⑤弁護士レビュー着手。2026-07-17に実装完了(`poc/src/cache.rs`/`signer.rs`/`signer/ed25519_signer.rs`/`main.rs`、`cargo test` 82 passed・`--features ed25519` 83 passed、脅威モデルレビュー実施済みでCritical/Highブロッカーなし)。実装後の既知の妥協点・S3持ち越し事項(reload非単調性・initial_tier既定値・版フラッディング・state.jsonノードローカル制約)は同ノート§13に記録。具体化された設計 → [S2.5_エントリ形式設計.md](./S2.5_エントリ形式設計.md) | [設計レビュー_2026-07.md](./設計レビュー_2026-07.md) §2 / §4.2 / §4.7 |
| 未採用の設計方針候補: 時刻アンカー(課題1関連) | 段階展開ではPhase2に繰り延べの個別機構。witness循環を断つ外部時刻アンカーの具体案 = 内蔵追記専用連鎖ログ+パブリックチェーン定期アンカーの二層((a)+(c))。Public Phase2で判断する候補のまま | [設計レビュー_2026-07.md](./設計レビュー_2026-07.md) §4.2 / §4.7 |
| 未採用の設計方針候補: 毒/Sybil対策 | 段階展開ではPhase2に繰り延べの個別機構。モデルコミットメント+評判ステーク+外部権威照合裁定の最小構成(情報=採掘/PoUW案の代替として検討)。Public Phase2で判断する候補のまま | [設計レビュー_2026-07.md](./設計レビュー_2026-07.md) §4.3 / §4.7 |
| 未採用の設計方針候補: 統治目標 | 幻覚パリティの概念(Tier分類)はPhase1から導入済み(Architecture §9 [補完])。強制機構(Tier-H裁定)はPublic Phase2で判断する候補のまま | [設計レビュー_2026-07.md](./設計レビュー_2026-07.md) §4.5 / §4.7 |
| S3(Company Phase1、実装完了)社内多ノード共有設計 | 中央レジストリ(発見専用)+ノード間直接配送・S2.5 load/verifyの受信側再実行・Company/Private起動時モード分離・組織PKIによるシビル不在の簡約(消すもの/消さないものの線引き)・Public Phase2への非破壊性(ポリシー差し替え点4点)を規定。S2.5 §13の残課題(reload非単調性・tier保守側反転・版フラッディング)との対応も明記。2026-07-18に`src/core/`+別クレート`registry`へ実装完了(`cargo test --workspace`が両ビルドで全緑、脅威モデルレビュー承認済み)。実装記録・既知制約・Phase2申し送り(H-1/M-1/M-2・CRL無署名配布の既知制約・revocationソース側フィルタ非対称)は同ノート§7・§8・§10に記録 | [S3_Company_Phase1_社内多ノード共有設計.md](./S3_Company_Phase1_社内多ノード共有設計.md) |
| S4(Company Phase1)層1内在信頼度先行設計(先行設計) | 3層評判のうち層1(独立生成回答のトリプル一致率=`trust.independent_agreement`/`supporting_versions`)のみを対象とするCompany Phase1先行設計。S3 §3手順9(`judge_entry`再実行)への「trust再導出ステップ」相乗り・集計単位(question_keyバンドル)・一致率メトリクス案A〜C(案A採用)・作用範囲を検索ランキング補助+UI表示の助言のみに限定(共有ゲートには配線しない)・実測ゲート(既定=重み0)・S3依存前提(仮リスト、S3実装確定後に差分再検証)・Public Phase2への非破壊性(policy hook化)を規定。層2/層3・witness独立性検証・revocation・ステークは範囲外(Public Phase2)。実装は承認後かつS3実装確定後の別段階(未着手) | [S4_Company_Phase1_層1内在信頼度先行設計.md](./S4_Company_Phase1_層1内在信頼度先行設計.md) |
