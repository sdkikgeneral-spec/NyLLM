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
| S4 評判・独立検証 | 3層評判・スラッシングの大半はPhase2繰り延べ。層1(トリプル一致率によるエントリ内在信頼度)相当のみPhase1で先行しうる(具体設計は [S4_Company_Phase1_層1内在信頼度先行設計.md](./S4_Company_Phase1_層1内在信頼度先行設計.md) を参照。層1は2026-07-19に先行実装完了 → §2 S4節) |
| S5 法的機構 | regurgitationフィルタ・revocationフラッディングのフル実装はPhase2繰り延べ(社内配布のみのCompany Phase1では法的圧力が相対的に低いため)。R2/R3/R4 3本柱の先行設計ノート(骨子+論点構造化。実装未着手・ゲート不変)を [S5_Public_Phase2_法的機構先行設計.md](./S5_Public_Phase2_法的機構先行設計.md) として追加済み |
| S6 モード分離+UI | Company/Private起動分離はPhase1で着手可。Public起動分離の本格運用はPhase2 |
| S7 Public限定公開 | 定義上Phase2専属(移行ゲート通過後) |

---

## 1. 実装ステップ全体表 (S1〜S7)

| 段階 | 内容 | ゲート(通過条件) | 現状ステータス |
|---|---|---|---|
| S1 PoC最小ループ | Embedding検索 → ミス時Agent呼び出し → 署名付きキャッシュ登録(単一ノード) | Hit/Missが動く | **完了** |
| S2 判定パイプライン | L0/L2ゲート+案4トリプル分解+揮発性初期付与 | §7フロー通過率を実測 | **完了** |
| S3 P2P化 | DHT分散・witness署名・複数版併存 | 2ノード以上で共有 | **完了(Company Phase1縮約範囲)**(注記: フルP2P〔DHT・witness〕は§0対応表どおりPublic Phase2で再拡張) |
| S4 評判・独立検証 | 3層評判・スラッシング・層3抜き打ち | 毒注入テストに耐える | **一部着手**(層1内在信頼度のみ先行実装完了〔2026-07-19〕。層2〔局所EigenTrust等〕・層3〔抜き打ち再推論〕はPublic Phase2据え置きであり、ゲート「毒注入テストに耐える」は層2/層3前提のため未達 → §2 S4節。ゲート定義自体は不変) |
| S5 法的機構 | regurgitationフィルタ・revocationフラッディング・出所記録 | 失効が全ノードに伝播 | **未着手**(注記: ソース側revocationフィルタ対称化〔供出/Digest列挙〕をS3実装の延長として2026-07-20に横断実装。**S5ゲート「失効が全ノードに伝播」は未達のまま・S5ステータス「未着手」は不変** → §2「S5 法的機構」節) |
| S6 モード分離+UI | Public/Company/Private分離・モード別起動 | 誤操作でPrivateが漏れない | **一部着手**(Company/Private起動分離のみS3実装内で先行実装。UI・Public分離・ゲート判定は未達 → §2 S6節) |
| S7 Public限定公開 | 招待制/小規模でフィルタ・失効を実証 | 弁護士レビュー通過 | **未着手** |

現在の最先端(直近で手を付けるべき段階): **S4 評判・独立検証(層1内在信頼度の先行実装)** — **層1先行実装は2026-07-19に完了**(段落末尾を参照。次の主戦場の選定は別途判断)。S2はゲート(§7フロー通過率を実測)を`bench_pipeline.rs::pipeline_flow_passrate`(代表質問12問の固定セット)で満たし、L2 Agent自己申告/案4トリプル分解/揮発性初期付与(§10.1)/§7フロー判定パイプラインの実装・テスト・脅威モデルレビューを2026-07-17に完了したため「完了」とした(詳細・実測値・残存リスクは下記S2節を参照)。**S3着手前に必須とされていた残存リスク(embedding署名対象化・受信側shareable再導出・provenance署名対象化)はS2.5(エントリ形式再設計)の実装完了(2026-07-17。上記§0[B]参照)により解消済み**。S2.5の脅威モデルレビューでCritical/Highの新規ブロッカーは確認されていないが、情報として記録された残存事項(reload時再導出の非単調性・initial_tier既定値・版フラッディング・state.jsonのノードローカル制約)は [S2.5_エントリ形式設計.md](./S2.5_エントリ形式設計.md) §13 にS3持ち越しとして記録済み。**S3(Company Phase1縮約範囲)は`src/core`+ 別クレート`registry`への実装・全テスト緑・脅威モデルレビュー承認を2026-07-18に完了したため「完了」とした(詳細は下記§2「S3 P2P化」節を参照)**。S4は層1(トリプル一致率によるエントリ内在信頼度)の先行実装を、先行設計ノート([S4_Company_Phase1_層1内在信頼度先行設計.md](./S4_Company_Phase1_層1内在信頼度先行設計.md))§2依存前提の差分再検証を経て**2026-07-19に完了した**(`cargo test --workspace` 111 passed・default / `--features ed25519` 両ビルド緑・ベンチ実施済み。→下記S4節)。ただしS4ゲート「毒注入テストに耐える」は層2(局所EigenTrust等)・層3(抜き打ち再推論)を前提とするため、S4ステータスは「一部着手」に留まる(層2/層3はPublic Phase2据え置き。→§0対応表)。Phase1で直近に残る候補は、層1実測ゲート有効化判断の社内実測(→S4節)・S6(UI層)・Phase1→Phase2移行ゲート5条件の充足(→§0)であり、次の主戦場の選定は別途判断とする。

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

**残存リスク(S3で受信answerを再分解する設計にする場合の追加要件)**: `decompose` の準二次スキャンに対し、敵対的長文入力の入力長・反復回数上限(DoS対策)を設ける。**追記(2026-07-20、再スコープ)**: S3実装は受信answerを再分解しない設計(Transferはcore+署名のみ・受信側は署名済み`facts`から`derive_operative_state`で再導出)となったため、本項の条件「受信answerを再分解する設計にする場合」は満たされず、現行では非該当(liveな脅威ではない)。詳細は§2 S3節「残課題」・§3の該当行(完全対応による解消ではなく、設計変更時に再浮上する条件付き課題としての格下げ)。

**S2として許容する安全側の残存(誤って過剰ブロックする方向=汚染ではない。将来課題)**:
- L0語彙の部分文字列一致による誤爆(英語 "now"⊂"know"、日本語「いま」⊂「います」)
- `contains_standalone_number` のMP3型(末尾数字)誤検知
- 漢数字・単位なし綴りの時事値がガードを素通りする

いずれも「疑わしきは共有しない」を過剰に効かせる方向であり毒混入方向ではないが、実埋め込み/多言語対応時に共有率を毀損しうるため、語彙・数値境界の精緻化を将来課題とする(下記§3の「述語オントロジーの初期構築方法と多言語対応」「共有用τの実測チューニング」と関連)。

### S3 P2P化 — 完了(Company Phase1縮約範囲)

**裏取り**: `src/core/`(`node.rs`/`transport.rs`/`wire.rs`/`sync.rs`/`registry_client.rs`/`daemon.rs`/`policy.rs`/`main.rs`)+ 別クレート `registry` の存在・実装内容を確認。`cargo test --workspace` が default / `--features ed25519` の両ビルドで **全緑**(**67 passed / 0 failed / 2 ignored**。実装コミット時点は66 passedで、その後の軽微なテスト追補により67で確定・両ビルド実測済み。2026-07-18)。S3ゲート判定テスト `s3_gate_two_or_more_nodes_share`(`src/tests/test_sync.rs`。3ノードで shareable エントリが全ノードに複製されることを assert)が両ビルドで緑であることを確認。脅威モデルレビュー(2026-07-18)実施済み・判定=承認(修正必須なし、production不変条件はすべて維持)。

**実装内容の要約**(具体設計は [S3_Company_Phase1_社内多ノード共有設計.md](./S3_Company_Phase1_社内多ノード共有設計.md) を参照):

- レジストリ発見(参加/離脱・ピア一覧・CA公開鍵/CRL配布のみを担う軽量`registry`クレート)+ ノード間直接HTTP配送(Announce/Request/Transfer/Digestの`nyllm-wire/v1`)。
- 受信側検証手順1〜10(エンベロープ解析 → 組織PKI検証〔`node_cert`のCA署名+CRL未失効〕→ ハッシュ照合 → 署名検証 → コアパース → `question_key`再計算 → embedding再計算 → 運用値再導出〔実装は`cache.rs::derive_operative_state`: 署名済み`core.facts`から`shareable`/`tier_operative`/`volatility_class_operative`を再導出。Transferにanswer平文が含まれないため`judge_entry`/`decompose`の再実行=answer再分解は行わない。設計ノート§3手順9の「`judge_entry`再実行」表記からの実装時調整、→更新履歴2026-07-19の調整点(d)〕 → 重複排除・冪等マージ)。送信者申告の`mutable_state`は一切信頼しない。
- 冪等マージ・複数版併存(`entry_id`単位のgrow-only集合。同一`question_key`×異`entry_id`は別版として併存)・anti-entropy(Digest定期交換による取りこぼし補償)。
- `--mode company` / `--mode private` の起動時モード分離(別`store_dir`、Privateは配送層〔transport/sync/registry_client〕を一切インスタンス化しない、レジストリ未登録で`/registry/peers`にも現れない)。
- Phase2への非破壊性を担保するポリシー差し替えフック4点(cert検証/時刻検証/失効フィルタ/発見層。`policy.rs`)。
- 脅威レビュー由来で先行実装した対処: **H-1**(CRL失効著者の既取込エントリを検索・供出・Digestの各パスで遡及除外。cert表に依存せず`author_pub`から`node_id`を直接再計算してCRL照合)、**M-1**(CA公開鍵のピン留め+TOFU。初回の非空CA供給で固定し、以後レジストリが偽CAを配布してもピンは上書きされない)、**M-2**(shareable単調性保護。reload時`shareable`を「再導出値 AND ディスク値」で合成し、非単調な反転〔false→true〕を防ぐ。回帰テスト`test_monotonic_shareable.rs`)。

**ゲート達成根拠**: ゲート条件「2ノード以上で共有」は`s3_gate_two_or_more_nodes_share`(3ノード構成でshareableエントリが全ノードに複製されることをassert)が default / `--features ed25519` の両ビルドで緑であることに加え、registry実HTTP統合スモーク(join/peers/ca/refresh_once、CAピン留め/TOFU、privateノードが`/registry/peers`に現れないこと)を含む全テストが緑であることをもって達成とする。

**残課題**(Phase2送り・注記):
- CRLはレジストリから無署名・平文HTTPで配布されるため、レジストリcompromise時にCRL検閲(正規ノードの失効注入)が成立しうる(Phase1既知制約)。Phase2でCA署名付きCRLにより対処する。
- `NodeConfig`のTTL(volatile=1時間、slow=30日)はPhase1暫定値としてコード側で設定されたものであり、確定チューニングは未対応のまま(§11-7とも関連)。
- `decompose`の入力長・反復回数上限(DoS対策)— **再スコープ(前提陳腐化。2026-07-20追記。完全対応による解消ではない)**: 実装済みの受信経路(`sync.rs::ingest_transfer` → `cache.rs::derive_operative_state`)は署名済み`core.facts`から運用値を再導出するのみで`decompose`を呼ばず(Transferにanswer平文が含まれない)、`decompose`が走るのは登録時=自ノードAgent出力(信頼境界内)のみ。よって「受信answer再分解によるDoS」は現行実装では成立しない(旧記述「依然未対応のまま=有効な課題」は前提が陳腐化)。将来、受信側が自由文answerを再分解する設計に変えた場合に再浮上する条件付き課題として、登録時`decompose`の全体入力長・文数上限の不在(自ノード出力のため優先度低)とあわせて下記§3の該当行に記録を残す。
- エントリ単位失効(`RevocationPolicy`)は受信側フィルタ(検索除外・anti-entropyプル前)のみに配線されており、供出/Digest列挙のソース側は著者CRLのみを参照する非対称がある。Phase2でエントリ失効の中身を実装する際、ソース側フィルタの要否を再評価すること。**追記(2026-07-20)**: 供出・Digest列挙の2経路は2026-07-20に配線済み(Phase1非破壊・既定no-op)。ingest経路(Announce起点の`handle_announce`→`ingest_transfer`)は grow-only原則によりあえて非配線=S5ノート§3(d)・OQ#14で決着(→下記「S5 法的機構」節横断注記)。

上記により、ゲート条件「2ノード以上で共有」を達成したため、本表(§1)のS3ステータスを「未着手」→「**完了(Company Phase1縮約範囲)**」に更新する。フルP2P(DHT・witness循環対策込み)はPublic Phase2で再拡張する(§0対応表どおり)。

**注記(2026-07-19、横断: 選択可能な推論先=Ollama対応の実装完了)**: Agent層に選択可能な推論先を、既存 `src/core/agent.rs` の拡張として実装完了した(`OllamaAgent` は `feature = "ollama"` 下でのみコンパイル、`src/core/agent/ollama_agent.rs`、transport は `ureq` 2系、既定モデル `gemma3`。バックエンドは環境変数 `NYLLM_AGENT_BACKEND=mock|ollama` 等の `AgentConfig` で選択)。`Agent::ask` は `Result<String, AgentError>` 化され、Agent失敗時は judge/登録/announce を行わない(daemon の `/v1/ask` は Timeout→504、それ以外→502)。`OllamaAgent::name()` は `ollama:<モデル名>` を返し provenance をモデル単位まで追跡可能(Architecture §11 R4整合)。テストは `src/tests/test_agent.rs`、`cargo test --workspace` 91 passed・default / `--features ed25519` 両ビルド緑。**これは新ステージの追加ではなくS1(Agent trait)〜S3(daemon/sync)を横断するAgent層拡張であり、S1〜S7の段階定義・ゲート・進捗ステータス(§1・§2)は変更しない。** 実装スペック: [superpowers/specs/2026-07-18-selectable-inference-backend-design.md](./superpowers/specs/2026-07-18-selectable-inference-backend-design.md)(冒頭「改訂注記(2026-07-19)」に実装時調整を記録)。なお、ローカル推論モデルの利用規約が Architecture §11 R5 で未整理である点(共有本格化前の要確認事項)は同スペック末尾追記・Architecture §11 [補完] を参照。

**注記(2026-07-20、横断: 共有由来エントリへのSHARED_THRESHOLD配線)**: 既知の穴②(→§3の該当行・Architecture §7[補完])を解消するため、`src/core/entry.rs::MutableState` に `origin_received: bool`(署名対象外・wire非搭載・ノードローカルstateのみ・既定`true`=保守側)を追加し、`cache.rs::effective_threshold`(共有由来=`SHARED_THRESHOLD` 0.90/自ノード登録=0.80)+lookupの`best_any`/`best_ok` 2本立てとして検索経路に配線した。TTL/失効の除外フックや`prefer_candidate`タイブレークとは独立に、最後のしきい値到達判定にのみ効く。実装+テスト+脅威モデルレビュー完了(`cargo test --workspace` **119 passed**・default / `--features ed25519` 両ビルド緑・ブロッカーなし)。**これはS3実装(`src/core`)の延長であり、S1〜S7の段階定義・ゲート・進捗ステータス(§1・§2の表)は変更しない。**

### S4 評判・独立検証 — 一部着手(層1内在信頼度のみ先行実装完了)

3層評判のうち**層1(エントリ内在信頼度)のみ2026-07-19に先行実装完了**(下記「層1先行実装の完了」)。層2(局所EigenTrust・ID生成PoW・スラッシング)・層3(消費側抜き打ち再推論)に対応するコードはなし(Public Phase2据え置き)。**ゲート「毒注入テストに耐える」は層2/層3を前提とするため未達であり、段階の目標としては未達=「一部着手」とする(ゲート定義自体は不変)。**

**注記(2026-07-17、S2.5実装後)**: `trust`フィールド(Architecture §6の`independent_agreement`等に相当)はS2.5により`MutableState`の**空スロット(Phase2予約。常に`None`)**としては存在するが、中身(評判集計ロジック)は未実装のままである([S2.5_エントリ形式設計.md](./S2.5_エントリ形式設計.md) §2)。**追記(2026-07-19)**: 層1先行実装により `independent_agreement`/`supporting_versions` の2フィールドは充填されるようになった(→下記「層1先行実装の完了」。`author_reputation`/`revoked` は引き続き空=Phase2)。

**注記(2026-07-17、S4層1先行設計ノート追加)**: 3層評判のうち**層1(トリプル一致率によるエントリ内在信頼度)のみを対象とするCompany Phase1先行設計ノート**を [S4_Company_Phase1_層1内在信頼度先行設計.md](./S4_Company_Phase1_層1内在信頼度先行設計.md) として追加した(層2/層3はPublic Phase2の範囲外のまま)。同ノートは「先行設計(docs)・実装は別段階(未着手)」であり、実装はS3(Company Phase1多ノード共有)の実装確定後に依存前提を差分再検証してから着手される。**追記(2026-07-18)**: S3実装が確定したため(→上記S3節)、同ノート§2「S3依存前提(仮リスト)」の差分再検証が実施可能になった(差分再検証自体は未実施。S4着手時の最初の作業)。**本表(§1)のS4ステータス「未着手」は変更しない**(当時)。**追記(2026-07-19)**: 差分再検証を実施の上、層1先行実装を完了。S4ステータスは「一部着手」へ更新(→下記「層1先行実装の完了」)。

**注記(2026-07-19、手続き的知識レーンの前提条件)**: 手続き的知識(プログラミング一般知識・コード)の共有レーンは、answer/コード本文の保存・伝播を要する**第二のエントリ形式**(S2.5の core=facts のみ設計の外側)であり、**S4の成果物(内在信頼度・検証証跡付き信頼度・witness再検証=N-of-Mサンドボックス機械検証)を前提条件とする将来レーン**である — **S4なしに着手しない**(本命根拠は需要でなく機械検証可能性。陳腐化はリリースイベント駆動失効、ティアはTier-H相当か専用ティア。一次記載は[信頼性設計メモ.md](./信頼性設計メモ.md) §5「手続き的知識」追記・プロダクト性格づけは同§9追記)。S3がCompany Phase1(社内限定)であることを活かした社内限定パイロットは検討事項(→§3)。**新ステージは追加せず、本表(§1)のS4ステータス・ゲート定義は変更しない。**

**層1先行実装の完了(2026-07-19)**

**裏取り**: `src/core/trust.rs`(新規。算出コア `compute_layer1_trust` を純粋関数として実装)/`entry.rs`(`Trust`型の `independent_agreement`/`supporting_versions` 2フィールド充填)/`policy.rs`(`TrustPolicy` trait+`Layer1TrustPolicy`。既定重み0=S3 §8の4点に続く**5点目のポリシー差し替え点**)/`cache.rs`(`recompute_trust_for_bundle`/`recompute_trust_all`・`lookup_filtered_weighted`)/`sync.rs`(受信・登録・起動時の再導出フック+`EntryDetail.trust`=API/UI表示用)/`lib.rs`(配線)の実装内容を確認。テストは `src/tests/test_trust.rs`(新規15件)+`test_policy_hooks.rs`(trustフィールド追補)を含め、`cargo test --workspace` が **111 passed / 0 failed / 4 ignored**(default)、`--features ed25519` でも **111 passed / 0 failed**(2026-07-19)。脅威モデルレビュー(2026-07-19)実施済み・重点6項目すべて問題なし(低1件・情報1件は下記「残存事項」)。

**実装内容の要約**(設計は [S4_Company_Phase1_層1内在信頼度先行設計.md](./S4_Company_Phase1_層1内在信頼度先行設計.md)、実装時の前提差分・調整は同ノート冒頭「改訂注記(2026-07-19)」を参照):

- 同ノート§9要判断点は**全5点とも推奨案どおり採用**(案A=正規化トリプル集合の版ペア間Jaccard平均 / `created`新しい順主軸+trustタイブレーク / 実測ゲート既定重み0 / `created`による簡易鮮度 / trust再導出はjudge・取込・起動と同時=同期実行)。
- 追加規約(実装で明文化): facts分解が成功している版が0または1(ペアが組めない)場合は `independent_agreement = 0.0`(「独立の裏づけ未取得」を全版一致1.0と保守的に区別)。
- §2依存前提の差分再検証の結果、調整点は(d)のみ: S3手順9は実装では `judge_entry` 再実行ではなく `cache.rs::derive_operative_state`(単一エントリ純粋関数)だったため、trust再導出は「版集合を持つ `SemanticCache` のバンドル再計算メソッド」として実装し、受信(`ingest_transfer` のAdded直後)・登録(askでの新規登録直後)・起動(`recompute_trust_all`)の各タイミングに相乗りさせた。設計意図(**受信側ローカル再導出・送信者値不信任=Transfer/`EntryEnvelope` はcore+署名のみで `trust` を運ばない**)は保持。
- 作用範囲は設計どおり**助言のみ**: 検索ランキング寄与は既定重み0(実測ゲート。有効化しきい値は社内実測待ち)、**共有ゲート(`shareable`)には一切配線していない**。
- ベンチ(同ノート§9-5「hot path負荷」、release実測): `compute_layer1_trust` はO(版数²)で k=2:約8µs / k=10:約74µs / k=50:約510µs / k=200:約5.2ms。`recompute_trust_for_bundle`(算出+バンドル全版のstate.json書き込み)はI/O支配で k=2:約0.47ms / k=10:約2.1ms / k=50:約11.6ms(典型バンドル k=2〜5版で約0.5〜1.1ms)。同期再導出(§9-5採用案)は現行規模で妥当と判断。**追記(2026-07-19、案B)**: このstate.json書き込みは案B(trust非永続化=`#[serde(skip)]`+`save_state`除去)で解消済み(k=50:約11.6ms→約0.15ms。→§3該当行)。

**残存事項(脅威モデルレビュー2026-07-19)**:
- **低**: 自作自演(単一著者による多版再注入)で `supporting_versions`/`independent_agreement` を水増しできる(重複版dedup未実装。同ノート§1非目標・§5同一モデル問題として据え置かれた限界そのもの。現状は重み0=助言のみ+組織PKIシビル不在前提で実害封じ込め済み)→ §3未解決事項に有効化前提条件として記録。
- **情報**: Phase2でpolicy hookをwitness独立性検証付きへ差し替える際、`TrustPolicy::compute` の出力域(0..1・非NaN)を契約として明文化することを推奨。

### S5 法的機構 — 未着手

regurgitationフィルタ・revocationフラッディングに対応するコードなし。**注記(2026-07-17、S2.5実装後)**: 出所記録は`immutable_core.provenance`(`agent`/`model`/`embedder_model_id`)としてS2.5で署名対象化・実装済みだが、これはPoCの生成主体メタデータの記録であり、S5が指すregurgitationフィルタ・revocationフラッディング・失効伝播の機構自体は引き続き未着手である([S2.5_エントリ形式設計.md](./S2.5_エントリ形式設計.md) §1)。

**注記(2026-07-19、S5先行設計ノート追加)**: **Public Phase2専属**のS5について、R2(regurgitationフィルタ)/R3(revocation)/R4(出所記録)3本柱の先行設計ノートを [S5_Public_Phase2_法的機構先行設計.md](./S5_Public_Phase2_法的機構先行設計.md) として追加した(「骨子+論点構造化」レベルの完成度であり実装可能な仕様書ではない)。R3(revocation伝播)についてはS3が残した既知課題(`RevocationPolicy`が受信側フィルタ〔検索除外・anti-entropyプル前〕のみに配線され、供出/Digest列挙のソース側は著者CRLのみを参照する非対称、→上記S3節「残課題」)への解消案(ソース側2経路にも`RevocationPolicy`を配線する)まで踏み込んだ。R2/R4は解空間・open questionsの整理に留める。**本表(§1)のS5ステータス「未着手」・ゲート「失効が全ノードに伝播」は変更しない。**実装着手可否はPublic Phase2移行判断(→§0「移行ゲート」)に委ねる別段階。

**注記(2026-07-20、横断: ソース側revocationフィルタ対称化を実装)**: S5ノート§3(d)が「要」と結論した供出(`handle_entry_request`)・Digest列挙(`handle_digest_request`)の2経路への`RevocationPolicy`配線を実装完了した(`src/core/sync.rs`、H-1〔ノード単位失効の遡及除外〕と同型のパターンをエントリ単位失効へ拡張。既定`NoRevocationPolicy`は常に`false`のためPhase1非破壊=既定no-op)。テスト5件追加、`cargo test --workspace` **124 passed**・default/`--features ed25519`両ビルド緑、脅威モデルレビュー実施済み(ブロッカーなし)。ingest経路(Announce起点)は grow-only原則により意図的に非配線のまま(S5ノート§4.1・§8 OQ#14で決着 → 上記「S3 P2P化」節「残課題」横断注記)。実装スペック: [superpowers/specs/2026-07-20-s5-source-side-revocation-filter-design.md](./superpowers/specs/2026-07-20-s5-source-side-revocation-filter-design.md)。**これはS3実装(`src/core`)の延長としてソース側revocationフィルタ対称化を横断実装したものであり、S5ゲート(「失効が全ノードに伝播」)は未達のまま、S5ステータス「未着手」・S1〜S7の段階定義・ゲート・進捗ステータス(§1・§2の表)は変更しない**(Revokeワイヤ型・tombstone集合・伝播機構はいずれも未実装)。

### S6 モード分離+UI — 一部着手

**Company/Private の起動時モード分離のみ、S3実装(2026-07-18)の一部として先行実装済み**(§0[C]「S6モード分離はPhase1で着手可」の範囲): `src/core/main.rs` の `--mode company|private` 起動分岐(別`store_dir`=`company_store`/`private_store`、Privateは配送層〔transport/sync/registry_client〕を一切インスタンス化しない)、および回帰テスト `test_registry_integration.rs::private_node_never_appears_in_registry`(privateノードがレジストリ`/registry/peers`に現れないこと)。詳細は上記S3節・[S3_Company_Phase1_社内多ノード共有設計.md](./S3_Company_Phase1_社内多ノード共有設計.md) §6。

一方、**Publicモードの分離(定義上Phase2)と、UI層(C# Blazor、CLAUDE.md記載の`src/ui/`)は未着手**(`src/ui/` ディレクトリ自体が存在しない)。ゲート条件「誤操作でPrivateが漏れない」の総合判定(UI操作経路を含む)も未実施のため、段階の目標としては未達=「一部着手」とする(ゲート定義自体は不変)。

**注記(2026-07-20、横断: 共有キルスイッチを実装)**: 共有キルスイッチ(CLI `--sharing on|off` + 実行中の `POST /v1/sharing` + `status` 応答への `sharing_enabled`/`sharing_active` 追加)を実装した。`src/core/sync.rs` の `NodeService` に `sharing_enabled`(`AtomicBool`、既定`true`=非破壊)を追加し、判定ヘルパ `sharing_active()`(`delivery.is_some() && sharing_enabled`)へ既存の送出系5経路(`broadcast_announce`/`handle_entry_request`/`handle_digest_request`/`run_anti_entropy_once`/`handle_announce`)の早期リターン条件を集約。`src/core/main.rs` にCLI引数 `--sharing`、`src/core/daemon.rs` のUI向け`/v1`名前空間に `POST /v1/sharing` と `status` 拡張を追加(`/wire/*` は変更なし)。既定安全側(ライブラリ既定は`sharing_enabled=true`のまま非破壊)だが、CLI起動時指定または実行中トグルにより**再起動不要**で即座に共有をオフにできる(トグル可逆)。UI(Blazor)が消費するためのHTTP API契約を`src/core`側に用意したのみで、**UI本体はS6継続のまま未着手**。テスト7件追加、`cargo test --workspace` **131 passed**・default/`--features ed25519`両ビルド緑、脅威モデルレビュー実施済み(送出経路の網羅性・private多層防御・トグルの並行性・データ漏洩経路のいずれもブロッカーなし)。実装スペック: [superpowers/specs/2026-07-20-sharing-killswitch-and-legal-posture-design.md](./superpowers/specs/2026-07-20-sharing-killswitch-and-legal-posture-design.md)。

キルスイッチの停止保証の**スコープ明確化**(脅威レビューM-1/M-2由来。新規ゲート・段階の追加ではなく現行実装範囲の記録): キルスイッチのオフが止めるのはcacheコンテンツの送出/取得5経路(announce・供出`handle_entry_request`・Digest列挙`handle_digest_request`・anti-entropy・`handle_announce`取り込み起動)である。レジストリへのプレゼンス公告(node_id/URL/node_certのjoin/refresh)はオフでも継続しうる(cache/回答データ自体は一切出ない=供出3経路が空を返すため無害)。すなわち「共有オフ」はネットワーク上のノード存在告知そのものを止めるものではない、というのが現行スコープである。また `/v1/*`(UI向けHTTP)はlocalhost/認証境界内での利用を前提とし、キルスイッチの停止保証は `/wire/*` コンテンツ経路に対するものである。`/v1/entries/{entry_id}` 等のローカル読み出し面はキルスイッチ・shareableの対象外(=リスナをlocalhost外に晒さない運用前提)。

これは**S1〜S7の段階定義・ゲート・進捗ステータス(§1・§2の表)を変更しない**。本件はS6(モード分離+UI)の延長強化であり、S6ゲート「誤操作でPrivateが漏れない」の趣旨を**強める**方向(既定安全側・実行中即オフ)である。あわせて法的姿勢の再定義(既定共有オフ・いつでも即オフ・弁護士レビューはPublic大規模運用主体への推奨へ格下げ)を [Architecture.md](./Architecture.md) §11・[信頼性設計メモ.md](./信頼性設計メモ.md) §8・[S5_Public_Phase2_法的機構先行設計.md](./S5_Public_Phase2_法的機構先行設計.md) 冒頭へ[補完]反映した(既存免責は削除・希薄化せず存置)。

### S7 Public限定公開 — 未着手

前提のうちS3はCompany Phase1縮約範囲で完了したが(フルP2PはPhase2再拡張待ち)、S4〜S6が未完了のため未着手。定義上Phase2専属(§0対応表)。§11の法的レビュー(専門弁護士レビュー)もまだ実施段階にない。

---

## 3. 未解決事項(旧 Architecture §13.1 より移植)

| 未解決事項 | 関連する主な段階 |
|---|---|
| Embeddingモデルの最終選定(MPNet級を推奨だが未確定)と共有用τの実測チューニング | S2(実測)/S3以降(実運用選定) |
| 述語オントロジー(揮発性クラス事前付与)の初期構築方法と多言語対応 **[補完: 元メモに具体設計なし。案4運用の前提として要設計]** | S2(案4トリプル分解の前提) |
| ✅ 受信ノードでの共有可否再判定(`shareable`を信頼せず再導出)を必須要件化 **[脅威レビュー(2026-07-17)。詳細はS2節「残存リスク(S3着手前に必須)」参照]** — **解消済み(2026-07-17、S2.5実装。§6手順8のロード時再導出=`derive_operative_state`が署名済み`facts`から`shareable`等を再導出。answer非保存のため`judge_entry`全段の文字どおりの再実行ではない点は2026-07-20に表現を精緻化)** | S3 |
| ✅ embedding の署名対象化・ロード時再計算(question由来) **[脅威レビュー(2026-07-17)。詳細は同上]** — **解消済み(2026-07-17、S2.5実装。embeddingは非保存・ロード時に再計算)** | S3 |
| ✅ provenance(agent)の署名対象化(Architecture §6完全準拠) **[脅威レビュー(2026-07-17)。詳細は同上]** — **解消済み(2026-07-17、S2.5実装。`provenance.agent`を`immutable_core`に含め署名対象化)** | S3 |
| `decompose` の入力長・反復回数上限(DoS対策。受信answerを再分解する設計にする場合) **[脅威レビュー(2026-07-17)。詳細は同上]** — **再スコープ(前提陳腐化。2026-07-20。完全対応による解消ではない)**: 実ソース裏取り(`src/core/sync.rs::ingest_transfer`/`cache.rs::derive_operative_state`)の結果、受信側は署名済み`core.facts`から運用値を再導出するのみで`decompose`を呼ばない(Transferはcore+署名のみを運びanswer平文を含まないため、受信answerの再分解は構造上発生しない)。`decompose`(`triples.rs`)が走るのは登録時=自ノードAgent出力(信頼境界内)のみ。よって「受信answer再分解によるDoS」は現行実装では前提が満たされず、**現行のliveな脅威ではない**。旧注記「受信側は`judge_entry`再実行で実際に再分解するため引き続き有効」は実装との不一致(ドリフト)であり撤回する。残置理由2点: (a) 将来、受信側が自由文answerを再分解する設計に変えた場合(例: 手続き的知識レーンの第二エントリ形式)に**再浮上する条件付き課題**、(b) 登録時`decompose`に全体入力長・文数上限が無い点は残る(`clean_fragment`のper-fragment上限〔30/50字〕のみ。入力が自ノードAgent出力のため優先度低)。→§2 S3節「残課題」の同項注記 | 現行では非該当(条件付き)。受信側が自由文answerを再分解する設計変更の時点で再評価を必須とする / 登録時の全体上限は低優先の任意堅牢化 |
| 局所EigenTrustの近傍サイズ・伝播ホップ数・スラッシング係数の具体パラメータ **[補完]** | S4 |
| regurgitationフィルタの参照コーパスをどう用意するか(著作物データベース非保有問題)**[補完]** | S5 |
| 誕生証明PoWの難易度調整(ネットワーク規模に応じた動的調整)**[補完]** | S4 |
| DHT実装の選定(既存Kademlia系流用可否)**[補完]** | S3(Company Phase1縮約ではDHT不採用=レジストリ発見で代替のため未着手のまま。フルP2P再拡張時=Public Phase2に持ち越し。→§0対応表) |
| ✅ 共有由来エントリへの `SHARED_THRESHOLD`(τ≥0.9)のlookup時配線 **[既知の穴②の記録(2026-07-19)。旧: 検索は出所を問わず一律 `LOCAL_THRESHOLD=0.80` で、共有由来エントリへの`SHARED_THRESHOLD`適用が存在しなかった]** — **解消済み(2026-07-20、実装+テスト+脅威モデルレビュー完了・ブロッカーなし)**: 「共有由来(shared-origin)」=他ノードから受信したエントリ(自ノード登録は対象外=0.80のまま)と定義し(根拠: 信頼性設計メモ §2脅威A「他人の別意図質問への誤ヒット」・Architecture §5.1「共有用τ≥0.9=精度優先」)、`src/core/entry.rs::MutableState` に `origin_received: bool` を追加(`#[serde(default)]` 既定 `true`=保守側。`register()`=false / `verify_envelope()`=true / `load()`手順9で`state.json`から復元、不在・破損時は`true`=0.90適用に倒す)。`cache.rs::effective_threshold` が共有由来=`self.threshold.max(SHARED_THRESHOLD)`/ローカル=`self.threshold` を適用し、lookupは `best_any`(観測用最良)/`best_ok`(実効しきい値を満たす候補内の最良)の2本立て。`origin_received` は署名対象外・`EntryEnvelope`(wire形式)非搭載・ノードローカルstateのみ(S2.5 §13「stateはノードローカル」「送信者値不信任」と整合)。詳細はArchitecture §7[補完]「既知の穴」 | 解消済み(S3実装の延長として`src/core`に配線)。鮮度加重等のさらなるクラス別制御はS4以降 |
| lookup時TTLの確定値チューニング **[既知の穴①の記録(2026-07-19)。設計原則=「登録時ゲートは情報を不可逆に捨てるため、TTL・クラス別τは検索時に強制する(後からチューニング可能)」。`poc/`のlookupはTTL機構なしのまま凍結(volatileも永久ヒット。PoC意図的縮約)。`src/core`はTTL検索除外(`sync.rs::is_searchable`)を実装済みだがTTL値(volatile=1時間/slow=30日)はPhase1暫定のまま=上記S3残課題と同一項目。上記SHARED_THRESHOLD配線(2026-07-20解消)とは別課題であり、こちらは社内実測待ちで継続。一次記載は信頼性設計メモ §5追記]** | S3(TTL確定チューニング=既存残課題。実測待ちで継続) |
| `state.json` の完全性保護(ローカルMAC等)の要否 **[将来課題の記録(2026-07-20)。`state.json`は非署名・ノードローカルのため、ローカルホストを完全掌握した攻撃者が `origin_received=false` へ改ざんすれば共有由来エントリのしきい値を0.90→0.80へ格下げできる(理論上)。これは「stateはノードローカル・署名対象外」設計(S2.5 §13)の受容済み帰結でありブロッカーではない(ホスト完全掌握時点でstore全体が改ざん可能)が、将来 `state.json` にローカルMAC等の完全性保護を入れるかは検討課題として残す]** | Phase2以降(優先度低。S2.5 §13「stateはノードローカル制約」と同根) |
| 手続き的知識(プログラミング一般知識・コード)の共有レーン(第二のエントリ形式・リリースイベント駆動失効・witness機械検証・Tier-H相当か専用ティア)**[S4成果物(内在信頼度・検証証跡)が前提条件=S4なしに着手しない。一次記載は信頼性設計メモ §5「手続き的知識」追記(2026-07-19)。→§2 S4節注記]** | S4以降(将来レーン。社内限定パイロットはCompany Phase1で検討可) |
| 層1 trust の自作自演(単一著者による多版再注入)による `supporting_versions`/`independent_agreement` 水増しが未dedup。**実測ゲート(ランキング重み)有効化 or Phase2 witness実装より前に「重複版dedup or witness時系列」を閉じることを有効化チェックリストに紐づける** **[脅威モデルレビュー(2026-07-19)・低。現状は重み0=助言のみ+組織PKIシビル不在前提で実害封じ込め済み。→§2 S4節「残存事項」]** | S4(実測ゲート有効化前)/ Public Phase2(witness) |
| ✅ `recompute_trust_for_bundle` はバンドル各版の state.json を毎回再書き込みする(O(版数)のI/O。release実測: k=2で約0.47ms〜k=50で約11.6ms)**[perf注記(2026-07-19)]** — **解消済み(2026-07-19、案B)**: `trust` を非永続化(`#[serde(skip)]`)し `save_state` を除去。trust は起動時 `recompute_trust_all` でローカル版集合から再導出される導出状態のため永続化不要(冗長I/Oだった)。release実測 k=2:約465µs→約2.3µs / k=10:約2.1ms→約15.9µs / k=50:約11.6ms→約0.15ms(I/Oが全体の約99%)。S4ノート§9-5代替案(非同期バックグラウンド再計算=案C)への切替は不要になった | S4(解消済み) |

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
- 2026-07-19: 設計議論(オーナー×Claude×fableレビュー)で確定した「キャッシュ適格性の層設計」と「既知の穴」をdocsへ書き起こし(docsのみ・コード変更なし)。信頼性設計メモ §5に「キャッシュ適格性の層設計(共有適格/永続ローカル適格/セッションスコープ、本質条件=回答が質問の安定な関数、Phase1二層現実解、S2.5補足事実、lookup時強制の設計原則)」「手続き的知識(コード)=S4前提の将来レーン(第二エントリ形式・機械検証可能性・リリースイベント駆動失効・専用ティア・slopsquatting・社内パイロット)」を追記、§9に「プロダクトの性格づけ(共有知識キャッシュを目指すが信頼機構の成熟度がカバー範囲を規定/雑談=原理的不適格とコード=時期尚早の区別)」を追記。Architecture §1・§7に対応する[補完]を追加(§7には既知の穴の実装状況表: `poc/`=lookupにTTL機構なしで凍結、`src/core`=TTL検索除外実装済みだがクラス別τ未配線・TTL暫定)。本ファイルは§2 S4節に「手続き的知識レーンの前提条件」注記、§3に既知課題2行を追加。段階定義・ゲート・進捗ステータス(§1・§2の表)は不変。
- 2026-07-19: 選択可能な推論先(Ollama対応)のAgent層拡張を実装完了(`src/core/agent.rs` 拡張+`src/core/agent/ollama_agent.rs`、`feature = "ollama"`、既定モデル`gemma3`、`Agent::ask`の`Result<String, AgentError>`化、テスト`src/tests/test_agent.rs`、`cargo test --workspace` 91 passed・両ビルド緑)。§2「S3 P2P化」節に横断注記を追加(新ステージは追加せず、S1〜S7の段階定義・ゲート・進捗ステータスは不変)。実装スペックは [superpowers/specs/2026-07-18-selectable-inference-backend-design.md](./superpowers/specs/2026-07-18-selectable-inference-backend-design.md)。あわせて Architecture.md §5.3(実装状況行)・§11(R5未整理事項の[補完])・信頼性設計メモ.md §8(同旨1行)・README(S3状態の整合修正+ローカル推論経路の記載)を更新。
- 2026-07-19: S5(Public Phase2)法的機構の先行設計ノート `docs/S5_Public_Phase2_法的機構先行設計.md` を新規作成(R2 regurgitationフィルタ/R3 revocation/R4 出所記録の3本柱を均等な記載粒度〔定義→設計案→open questions〕で整理する「骨子+論点構造化」レベルの先行設計。R3は既存の空スロット〔`witness_sigs`/`anchor_proof`/`stake`/`trust`〕・ドメインタグ`nyllm/revocation/v1`〔S2.5予約〕・S3の`policy.rs`の`RevocationPolicy`フックへ接続する設計まで具体化し、特にS3が残したソース側フィルタ非対称〔供出/Digest列挙が著者CRLのみ参照〕の解消案を提示。R2/R4は解空間・open questionsの整理に留める)。本ファイル §0対応表のS5行・§2「S5 法的機構」節・§5に本ノートへのリンクを追加。**S5自体の段階定義・ゲート(「失効が全ノードに伝播」)・進捗ステータス(§1、「未着手」)は不変**。実装着手可否はPublic Phase2移行判断に委ねる別段階として未着手のまま。
- 2026-07-19: S4層1(内在信頼度)の先行実装+テスト+ベンチを完了。[S4_Company_Phase1_層1内在信頼度先行設計.md](./S4_Company_Phase1_層1内在信頼度先行設計.md) §2依存前提の差分再検証(調整点は(d)のみ=S3手順9が`cache.rs::derive_operative_state`〔単一エントリ純粋関数〕として実装されていたため、trust再導出を「版集合を持つ`SemanticCache`のバンドル再計算メソッド」として受信・登録・起動の各タイミングに相乗り。受信側ローカル再導出・送信者値不信任の設計意図は保持)を経て、`src/core/trust.rs`(新規)/`entry.rs`/`policy.rs`(`TrustPolicy` trait+`Layer1TrustPolicy`=既定重み0の5点目policy hook)/`cache.rs`/`sync.rs`/`lib.rs`へ実装。テストは`src/tests/test_trust.rs`(新規15件)+`test_policy_hooks.rs`追補で`cargo test --workspace` **111 passed / 0 failed / 4 ignored**(default)、`--features ed25519`でも**111 passed / 0 failed**。ベンチ(同ノート§9-5)実施(算出コアはO(版数²)でk=50:約510µs、バンドル再計算はI/O支配でk=50:約11.6ms)。同ノート§9要判断点は全て推奨案どおり採用、ペア不在時`independent_agreement=0.0`規約を明文化。脅威モデルレビュー実施済み(重点6項目問題なし。低=自作自演多版再注入の未dedup、情報=`TrustPolicy::compute`出力域の契約明文化推奨)。§1のS4ステータスを「未着手」→「**一部着手**」(層1のみ先行実装完了。ゲート「毒注入テストに耐える」は層2/層3前提のため未達・**ゲート定義不変**)に更新し、§2 S4節に「層1先行実装の完了」節を追記、§3に自作自演未dedup行+perf注記行を追加。S4ノート冒頭に「改訂注記(2026-07-19)」を追加し、Architecture.md §5.4/§6差異表/§8冒頭に[補完]で反映。層2/層3・S1〜S3/S5〜S7の段階定義・ゲート・進捗ステータスは変更なし。
- 2026-07-19: S4層1 trust のI/O最適化(案B=trust非永続化)を実装。`entry.rs` の `MutableState.trust` を `#[serde(skip)]` 化し、`cache.rs::recompute_trust_for_bundle` の `save_state` を除去(trust は起動時 `NodeService::new`→`recompute_trust_all` でローカル版集合から再導出される導出状態のため、`state.json` への永続化は冗長I/Oだった)。ベンチ(release)で `recompute_trust_for_bundle` k=2:約465µs→約2.3µs / k=10:約2.1ms→約15.9µs / k=50:約11.6ms→約0.15ms(I/Oが約99%)を実測し、§9-5代替案(非同期化=案C)は不要と判断。`cargo test --workspace` は default / `--features ed25519` 両ビルドで **112 passed / 0 failed / 4 ignored**。脅威モデルレビューは不変条件4点維持・新規攻撃面なし(むしろ `state.json` 改竄による trust 吊り上げ経路が消え安全側)。本ファイル§2 S4節ベンチ記述・§3 perf注記行を「解消済み」に更新、S4ノート「改訂注記」・S2.5 §2・S3ノート§8・信頼性設計メモ§9に同旨を追記。段階定義・ゲート・進捗ステータス(§1・§2の表、S4=「一部着手」)は不変。
- 2026-07-20: 共有由来エントリへの `SHARED_THRESHOLD`(0.90)のlookup時配線を実装完了(既知の穴②の解消)。「共有由来」=他ノードから受信したエントリ(自ノード登録は0.80のまま)と定義し(根拠: 信頼性設計メモ §2脅威A・Architecture §5.1・§7[補完]既知の穴②)、`src/core/entry.rs::MutableState` に `origin_received: bool` を追加(`#[serde(default)]` 既定`true`=保守側。`register()`=false / `verify_envelope()`=true / `load()`手順9で`state.json`から復元、不在・破損時は`true`=0.90適用)。`cache.rs` に `effective_threshold`(共有由来=`self.threshold.max(SHARED_THRESHOLD)`)を新設し、lookupを`best_any`(観測用最良)/`best_ok`(実効しきい値を満たす候補内の最良)の2本立てへ変更(TTL/失効除外フック・`prefer_candidate`タイブレークとは独立)。`origin_received`は署名対象外・`EntryEnvelope`(wire)非搭載・ノードローカルstateのみ(S2.5 §13と整合)。`cargo test --workspace` **119 passed**(default / `--features ed25519` 両ビルド緑)、脅威モデルレビュー実施済み(ブロッカーなし)。本ファイルは§2 S3節に横断注記を追加、§3の旧「lookup時のTTL・クラス別しきい値の強制」行を「✅ SHARED_THRESHOLD配線=解消済み」と「lookup時TTLの確定値チューニング=実測待ちで継続」の2行に分割し、将来課題「`state.json`の完全性保護(ローカルMAC等)の要否」(非署名stateの`origin_received`改ざんによる0.90→0.80格下げはホスト完全掌握前提の受容済み帰結)を新規1行として追加。あわせて Architecture.md §7[補完]既知の穴表・§6差異表、信頼性設計メモ.md §5追記、S2.5_エントリ形式設計.md §2/§6/§13、S3_Company_Phase1_社内多ノード共有設計.md §3、PoC_Design_Notes.md §2 を同旨に更新。段階定義・ゲート・進捗ステータス(§1・§2の表)は不変。
- 2026-07-20: docsのみの再スコープ(オーナー承認済み・コード変更なし)。§3「`decompose`の入力長・反復回数上限(DoS対策)」行を、実ソース裏取り(`src/core/sync.rs::ingest_transfer`は`judge_entry`/`decompose`を呼ばず、`cache.rs::derive_operative_state`が署名済み`core.facts`から運用値を再導出=Transferにanswer平文が含まれない)に基づき「現行のliveな脅威ではない(前提陳腐化)」へ格下げ・再スコープ。**完全対応による解消ではない**ため✅は付けず、(a)受信側が自由文answerを再分解する設計へ変えた場合に再浮上する条件付き課題として残置、(b)登録時`decompose`の全体入力長・文数上限の不在(`clean_fragment`のper-fragment上限のみ。自ノードAgent出力のため優先度低)を明記。旧注記「受信側は`judge_entry`再実行で実際に再分解するため引き続き有効」は実装との不一致(ドリフト)として撤回。あわせて§2 S2節「残存リスク」・S3節「残課題」・S3節実装要約の「`judge_entry`再実行」表現・§3受信側再判定行の同表現(いずれも実装は`derive_operative_state`)、[PoC_Design_Notes.md](./PoC_Design_Notes.md) §5-6/§6、[S3_Company_Phase1_社内多ノード共有設計.md](./S3_Company_Phase1_社内多ノード共有設計.md) §3手順9注記を同旨に整合。段階定義・ゲート・進捗ステータス(§1・§2の表)は不変。
- 2026-07-20: S5ノート§3(d)が「要」と結論したソース側revocationフィルタ対称化(供出`handle_entry_request`・Digest列挙`handle_digest_request`の2経路への`RevocationPolicy`配線)を実装完了(`src/core/sync.rs`、H-1と同型の遡及除外パターンをエントリ単位失効へ拡張。既定`NoRevocationPolicy`は常に`false`のためPhase1非破壊=既定no-op)。テスト5件追加(`src/tests/test_revocation.rs`)、`cargo test --workspace` **124 passed**・default/`--features ed25519`両ビルド緑、脅威モデルレビュー実施済み(ブロッカーなし)。ingest経路(Announce起点)は grow-only原則により意図的に非配線のまま。実装スペック: [superpowers/specs/2026-07-20-s5-source-side-revocation-filter-design.md](./superpowers/specs/2026-07-20-s5-source-side-revocation-filter-design.md)。docs反映として、[S5_Public_Phase2_法的機構先行設計.md](./S5_Public_Phase2_法的機構先行設計.md) §3(d)に実装記録・§7非破壊性に実測結果・§8総覧のOQ#5(専用ベンチ不要=実装により解消)/OQ#14(配線しない=決着)を更新(他OQは現状維持)、本ファイル§2「S3 P2P化」節「残課題」・§2「S5 法的機構」節・§1 S5行に横断注記を追加。**これはS3実装の延長としてのソース側revocationフィルタ対称化であり、S5ゲート「失効が全ノードに伝播」は未達のまま、S5ステータス「未着手」・S3ステータス「完了(Company Phase1縮約範囲)」・S1〜S7の段階定義・ゲート・進捗ステータス(§1・§2の表)はすべて不変**(Revokeワイヤ型・tombstone集合・伝播機構は未実装)。

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
| S4(Company Phase1)層1内在信頼度先行設計(実装完了) | 3層評判のうち層1(独立生成回答のトリプル一致率=`trust.independent_agreement`/`supporting_versions`)のみを対象とするCompany Phase1先行設計。S3 §3手順9(`judge_entry`再実行)への「trust再導出ステップ」相乗り・集計単位(question_keyバンドル)・一致率メトリクス案A〜C(案A採用)・作用範囲を検索ランキング補助+UI表示の助言のみに限定(共有ゲートには配線しない)・実測ゲート(既定=重み0)・S3依存前提(仮リスト、S3実装確定後に差分再検証)・Public Phase2への非破壊性(policy hook化)を規定。層2/層3・witness独立性検証・revocation・ステークは範囲外(Public Phase2)。**2026-07-19に§2差分再検証(調整点(d))を経て`src/core`へ実装完了**(`cargo test --workspace` 111 passed・両ビルド緑・ベンチ・脅威モデルレビュー済み。実装記録は同ノート冒頭「改訂注記(2026-07-19)」・本ファイル§2 S4節)。S4ステータスは層2/層3据え置きのため「一部着手」(ゲート定義不変) | [S4_Company_Phase1_層1内在信頼度先行設計.md](./S4_Company_Phase1_層1内在信頼度先行設計.md) |
| S5(Public Phase2)法的機構先行設計(先行設計・骨子+論点構造化) | R2 regurgitationフィルタ/R3 revocation/R4 出所記録の3本柱を対象とするPublic Phase2専属の先行設計。R3(revocation伝播)は既存空スロット(`witness_sigs`/`anchor_proof`/`stake`/`trust`)・予約済みドメインタグ`nyllm/revocation/v1`(S2.5)・S3の`RevocationPolicy`フックへの接続、およびS3が残したソース側フィルタ非対称(供出/Digest列挙が著者CRLのみ参照)の解消案(受信側と同じ`RevocationPolicy`をソース側2経路にも配線)まで具体化。R2はコーパス調達問題(著作物DB非保有)の解空間3案を整理(結論は出さない、本領は将来の手続き的知識レーン=S4依存)。R4は`provenance`署名対象化済み(S2.5)を土台に`regurgitation_check`の`schema_ver`拡張案を整理。R2/R4はopen questionsが支配的。実装着手可否はPublic Phase2移行判断に委ねる別段階(未着手) | [S5_Public_Phase2_法的機構先行設計.md](./S5_Public_Phase2_法的機構先行設計.md) |
