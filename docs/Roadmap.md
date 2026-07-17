# Winny型 Semantic Cache — 実装ロードマップ

> 出典: 本ファイルは [Winny_Type_Semantic_Cache_Architecture.md](./Winny_Type_Semantic_Cache_Architecture.md) 旧§13.2「実装ステップ」+ 旧§13.1「未解決事項」+ 旧§13.3「PoC最小ループ(S1詳細)」を抽出・統合したもの。
> 二重管理回避のため、**実装ステップの内容(段階/ゲート)と進捗ステータスの一次情報はこのファイルのみ**とする。Architecture.md 側は参照リンクのみを残す。
> 前提ドキュメント: [Architecture.md](./Winny_Type_Semantic_Cache_Architecture.md) / [信頼性設計メモ.md](./Winny_Type_Semantic_Cache_信頼性設計メモ.md) / [PoC_Minimal_Loop.md](./PoC_Minimal_Loop.md)
> 作成日: 2026-07-17
> ステータス凡例: **未着手** / **一部着手**(範囲の一部のみ着手・段階の目標としては未達) / **一部完了**(主要機能は動くが残作業あり) / **完了**(ゲート通過)
> ステータスは `poc/`(および将来の `src/core`, `src/ui`)の実ソースを確認して記載している。裏取り方法は各段階の節を参照。

---

## 1. 実装ステップ全体表 (S1〜S7)

| 段階 | 内容 | ゲート(通過条件) | 現状ステータス |
|---|---|---|---|
| S1 PoC最小ループ | Embedding検索 → ミス時Agent呼び出し → 署名付きキャッシュ登録(単一ノード) | Hit/Missが動く | **一部完了** |
| S2 判定パイプライン | L0/L2ゲート+案4トリプル分解+揮発性初期付与 | §7フロー通過率を実測 | **一部着手**(L0のみ。S1 PoC内に先行実装済み) |
| S3 P2P化 | DHT分散・witness署名・複数版併存 | 2ノード以上で共有 | **未着手** |
| S4 評判・独立検証 | 3層評判・スラッシング・層3抜き打ち | 毒注入テストに耐える | **未着手** |
| S5 法的機構 | regurgitationフィルタ・revocationフラッディング・出所記録 | 失効が全ノードに伝播 | **未着手** |
| S6 モード分離+UI | Public/Company/Private分離・モード別起動 | 誤操作でPrivateが漏れない | **未着手** |
| S7 Public限定公開 | 招待制/小規模でフィルタ・失効を実証 | 弁護士レビュー通過 | **未着手** |

現在の最先端(直近で手を付けるべき段階): **S1の残作業(テスト未整備)を先に解消し、その後S2の残機能(L2/案4/揮発性初期付与)に着手する**のが筋。S1のゲート自体(Hit/Missが動く)は`cargo run`のデモで満たされているが、CLAUDE.md実装ワークフロー規則4(コンポーネント実装時はテストも実装する)が未充足のため「完了」ではなく「一部完了」とした。

---

## 2. 各段階の詳細

### S1 PoC最小ループ — 一部完了

**裏取り**: `poc/src/{embedder,signer,agent,cache,volatility,main}.rs` の存在確認、`poc/Cargo.toml`(依存: serde/serde_json/sha2/hex/rand/chrono、featureで ed25519/onnx)、`poc/src/tests/` ディレクトリの不存在を確認(2026-07-17時点)。コミット b09d826 でC++からRustへ書き直し済み。

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
- `main.rs` の6問デモでHit/Miss/改ざん検知の一連ループが動作確認済み(`docs/PoC_Minimal_Loop.md` §5)

**残作業(次にすべきこと)**:
1. **`poc/src/tests/` が存在しない。** CLAUDE.mdの実装ワークフロー規則4「コンポーネント実装時はテストも実装する」が未充足。少なくとも以下の単体テストが必要:
   - `cache.rs`: entry_id算出の一致性、`verify()`のhash不一致/署名不一致それぞれでの読み捨て動作
   - `volatility.rs`: `classify_volatility`の時間指示語判定、`share_gate`のAND条件(文脈依存語/主観語/個人参照/volatileそれぞれ単独での不可判定、全通過時のみ可)
   - `signer.rs`: DummySignerの署名/検証ラウンドトリップ、鍵ファイル読み書き
2. witness署名は「自己1件で仮」(単一ノードのため構造上の意図的な省略。S3で実装)— 対応不要、意図的縮約として明記済み
3. `README.md`に記載のテスト実行手順(`cargo test`)がまだ存在しない

### S2 判定パイプライン — 一部着手(L0のみ)

**裏取り**: `poc/src/volatility.rs` を読了。コメントに「permanent昇格は案4=知識グラフ分解が必要なためPoCでは行わない」「L2 Agent自己申告は未実装」と明記されている。`poc/src/cache.rs`の`CacheEntry.answer`も平文回答のみでトリプル型(`facts: [{s,p,o}]`)ではない(Architecture §6のデータモデルに対する意図的縮約、`PoC_Minimal_Loop.md` §6 に明記あり)。

**実装済み(S1内で先行)**:
- L0語彙ルール: 時間指示語→`volatile`、それ以外→`slow`(`permanent`分類は不在)
- 共有可否ANDゲート: 文脈依存語・主観語・個人参照・volatileのいずれかで不可、全通過時のみ可(デフォルト非共有の保守的設計は踏襲)

**未実装(S2で残っている作業)**:
1. **L2 Agent自己申告**: 「前提会話なしで単独回答できるか」「事実型か」「volatility」をAgentに申告させ、決定でなく一票として扱う仕組み(Architecture §7.3)
2. **案4 知識グラフ分解(トリプル分解)**: 回答を`(s, p, o)`へ分解し、述語オントロジーで`permanent`/`slow`/`volatile`を判定する主軸ロジック(現状は時間指示語の有無だけのL0代替に留まる)
3. **揮発性初期付与ルール(§10.1)**の完全実装: 現状は「時間指示語あり→volatile / それ以外→slow」の2値のみで、「分解成功かつpermanent型述語→permanent」のケースが存在しない
4. **§7フロー通過率の実測**(ゲート条件)がまだ行われていない

### S3 P2P化 — 未着手

DHT分散・witness署名・複数版併存はいずれも `poc/` に該当コードなし(`witness_sigs`は`CacheEntry`に存在せず、`PoC_Minimal_Loop.md`のデータモデル対応表にも記載なし)。トップレベル `src/core/`, `src/ui/`(CLAUDE.md記載の将来レイアウト)も未作成(2026-07-17時点でリポジトリに存在しないことを確認)。

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
| 局所EigenTrustの近傍サイズ・伝播ホップ数・スラッシング係数の具体パラメータ **[補完]** | S4 |
| regurgitationフィルタの参照コーパスをどう用意するか(著作物データベース非保有問題)**[補完]** | S5 |
| 誕生証明PoWの難易度調整(ネットワーク規模に応じた動的調整)**[補完]** | S4 |
| DHT実装の選定(既存Kademlia系流用可否)**[補完]** | S3 |

---

## 4. 更新履歴

- 2026-07-17: 新規作成。Architecture.md §13.1/§13.2/§13.3 を移植し、poc/ の実ソースを確認した上で各段階のステータスを付与。
