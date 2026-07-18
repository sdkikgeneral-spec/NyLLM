# 設計: 選択可能な推論先(Ollama対応)— `src/core/` Agent層

- 日付: 2026-07-18
- ステータス: 設計確定(実装未着手)
- 関連: Architecture.md §7.3 / §5 / §10.1, 信頼性設計メモ §10, Roadmap S3, [poc/src/agent.rs](../../../poc/src/agent.rs)

## 1. 背景と狙い

現状、キャッシュミス時の推論先は `MockAgent`(固定知識ベース)しかなく、実LLMは
`Agent` trait の拡張点として口だけ用意されている([poc/src/agent.rs](../../../poc/src/agent.rs))。

**狙い(adoption)**: キャッシュミス時の推論先を「設定で選べる」ようにし、Ollama 経由の
ローカルLLM(GLM / Gemma など)を誰でもすぐ差し込めるようにする。Ollama はローカルで
動くため、外部APIキー不要で「触ったらローカルLLMが実際に答える」体験を提供でき、
PoC を試す人の裾野を広げる。

「推論先を差し替え可能にする」こと自体は他のLLMツールでも一般的な枯れたパターンであり、
実証フェーズは省いて本命の置き場(`src/core/`)に直接作る、という判断。

## 2. スコープ

### やること(① 推論先のプラグイン化 = outbound)
- `src/core/` の**最初のモジュール**として Agent 層を建てる。
- 設定(環境変数)で推論先を `mock` | `ollama` から選べるファクトリ。
- `OllamaAgent` 新規実装(HTTP、モデル/エンドポイント/タイムアウト設定可能)。
- 実LLMによる `self_declare()`(自己申告)経路を初めて実現(失敗時はヒューリスティックに
  フォールバック)。
- 上記すべてのテスト。

### やらないこと(将来枠 / 別タスク)
- **② 既存エージェント基盤への組み込み(inbound)**: LangChain / Claude Code 等から
  NyLLM をセマンティックキャッシュ層として呼ばせる方向。①のデーモン化が入れば
  その「内向き口」になる、という位置づけだけ残す。今回は作らない(YAGNI)。
- キャッシュ / 署名 / 揮発性本体の `poc/` → `src/core/` 移植。別タスク(後続)。
- TOML 設定ファイルローダ。環境変数解決で始め、後で同じ `AgentConfig` を埋める薄い
  ローダを被せられる形にだけしておく。

### 全体像(①と②は同じパイプの両端)

```
既存フレームワーク ──(②inbound: 将来)──▶ NyLLMコア ──キャッシュ検索──▶ ヒット: 即返す
                                                └─ミス──(①今回)──▶ Ollama (GLM/Gemma)
```

## 3. モジュール構成

```
src/core/agent/
  mod.rs        Agent trait + SelfDeclaration + AgentError + create_agent(config) ファクトリ
  config.rs     AgentConfig(backend / model / endpoint / timeout)+ 環境変数解決
  mock.rs       MockAgent(poc から移植。ask は常に Ok)
  ollama.rs     OllamaAgent(新規): HTTP呼び出し / self_declare(B) / パース純関数
  heuristic.rs  self_declare フォールバック用のL0語彙ヘルパー(poc/volatility.rs から必要分を移植)
```

`heuristic.rs` は**暫定の継ぎ目**。将来 `volatility.rs` を `src/core/` へ移植する際に統合する
(重複を残さない)。この意図をファイル冒頭コメントに明記する。

Agent 層はキャッシュ/署名に依存せず単体で成立するため、空の `src/core/` を建て始める
最初の一歩として適切な単位。

## 4. Agent trait(改訂)

`ask` の返り値を `Result` に変更する。実LLMは失敗しうる(未起動/未pull/タイムアウト)ため、
失敗を型で表現する。MockAgent も移植時に合わせる(Mock は常に `Ok`)。

```rust
pub trait Agent
{
    fn name(&self) -> &str;
    fn ask(&self, question: &str) -> Result<String, AgentError>;
    fn self_declare(&self, question: &str, answer: &str) -> SelfDeclaration;
}

#[derive(Debug)]
pub enum AgentError
{
    // Ollama デーモンに到達できない(未起動 / エンドポイント誤り)
    Unreachable(String),
    // HTTP は通ったが非 2xx(例: モデル未pull)
    Http { status: u16, body: String },
    // タイムアウト
    Timeout,
    // レスポンス JSON のパース失敗
    Parse(String),
}
```

`ask` は同期のまま(trait に async を波及させない)。呼び出し側(将来のミス処理パイプライン)は
`AgentError` を見てハンドリング(リトライ/別バックエンド/エラー表示)を選べる。

`SelfDeclaration` は poc から不変で移植(`context_independent` / `factual` / `volatility`)。

## 5. 設定機構(「選べる」の実体)

`AgentConfig` を環境変数から解決(依存ゼロ):

| 環境変数 | 意味 | 既定 |
|---|---|---|
| `NYLLM_AGENT_BACKEND` | `mock` \| `ollama` | `mock` |
| `NYLLM_OLLAMA_MODEL` | Ollama モデル名(`glm4` / `gemma2` 等) | `gemma2` |
| `NYLLM_OLLAMA_ENDPOINT` | Ollama エンドポイント | `http://localhost:11434` |
| `NYLLM_OLLAMA_TIMEOUT_SECS` | リクエストタイムアウト(秒) | `60` |

```rust
pub fn create_agent(config: &AgentConfig) -> Box<dyn Agent>
{
    match config.backend
    {
        Backend::Mock   => Box::new(MockAgent),
        Backend::Ollama => Box::new(OllamaAgent::new(config)),
    }
}
```

不正な `NYLLM_AGENT_BACKEND` 値は既定(`mock`)にフォールバックし、警告ログを出す
(誤設定で起動不能にしない)。

将来: TOML `nyllm.toml` から同じ `AgentConfig` を埋める薄いローダを被せる余地を残す。

## 6. OllamaAgent(推論先)

- **transport**: `ureq`(同期・軽量)を `feature = "ollama"` の下で依存に追加。既定ビルドは
  依存を増やさない(`OllamaAgent` は feature 有効時のみコンパイル)。
- **API**: `POST {endpoint}/api/chat`、`stream: false` で 1 リクエスト 1 レスポンス。
  - リクエスト: `{ "model": <model>, "messages": [{"role":"user","content":<question>}], "stream": false }`
  - レスポンス: `.message.content` を回答として取り出す。
- **リクエスト組み立て / レスポンス抽出は純関数に切り出す**(`build_chat_request` /
  `extract_answer`)。HTTP I/O と分離してユニットテスト可能にする。
- **エラー処理**: 到達不能 → `Unreachable`、非2xx → `Http`、タイムアウト → `Timeout`、
  ボディJSONパース失敗 → `Parse`。

## 7. self_declare(B: モデル自己申告)

1. `ask` で回答生成後、**2回目のプロンプト**で構造化申告を要求する。
   - 申告JSONスキーマ: `{ "context_independent": bool, "factual": bool, "volatility": "permanent"|"slow"|"volatile" }`
   - プロンプトは「質問」「生成した回答」を与え、上記JSONのみを返すよう指示。
2. JSON パース成功 & 値が妥当 → その申告を `SelfDeclaration` として採用。
3. 失敗(HTTPエラー / パース不能 / 不正な volatility 値)→ **`heuristic.rs` のL0
   ヒューリスティックにフォールバック**(現 MockAgent の申告ロジックと同等)。
4. **不変条件(維持)**: LLM 申告は信頼できない前提。受け取り側(finalize_volatility /
   判定パイプライン)は §10.1 ルール4 通り、申告を**安全側にのみ**反映する。Yes/permanent
   側の申告が L0 や案4 の判定を覆すことはない。self_declare の実装変更でこの不変条件を
   壊さない。

`heuristic.rs` のフォールバックは volatile 疑い時に安全側(slow / non-factual)へ倒す
既存モックの方針を踏襲する。

## 8. テスト(CLAUDE.md ルール4)

`src/core/tests/test_agent.rs`(または `src/core/agent/` 内 `#[cfg(test)]`):

- **純関数**: `build_chat_request` が期待JSONを生成する / `extract_answer` が
  正常レスポンスから content を取り出す / 壊れたレスポンスで `Parse` を返す。
- **self_declare**: 妥当な申告JSONのパース / 不正JSON → ヒューリスティックフォールバック /
  不正 volatility 値 → フォールバック。
- **config**: 環境変数の解決 / 不正 backend 値 → mock フォールバック / 既定値。
- **不変条件**: LLM が permanent と申告しても、安全側にしか反映されないことを
  受け取り側テストで確認(該当ロジックが core にまだ無い場合は self_declare 出力の
  検証に留め、パイプライン移植時に拡張と明記)。
- **統合(ライブ Ollama)**: `#[ignore]` 付き。ローカルで `cargo test --features ollama -- --ignored`。
  CI では走らせない。

## 9. ビルド

```sh
cargo build                      # 既定: MockAgent のみ。ureq 依存なし
cargo build --features ollama    # OllamaAgent 有効(ureq 追加)
cargo test                       # ライブサーバ不要のテスト
cargo test --features ollama -- --ignored   # 実 Ollama 疎通
```

`Cargo.toml`: `ureq` を `optional = true` にし、`[features] ollama = ["dep:ureq", ...]`。
JSON は既存の `serde` / `serde_json` を利用。

## 10. コーディング規約(CLAUDE.md)

- コアは Rust、`src/core/` 配下(poc のフラット構成は踏襲しない)。
- コメントは日本語。
- Allman ブレース(手動維持。`cargo fmt` をブレース目的で無差別実行しない)。
- コード内の設計参照は番号でコメント(例 `Architecture §7.3`)。

## 11. 未解決 / 継ぎ目(実装時に留意)

- `heuristic.rs` は `volatility.rs` の core 移植時に統合(重複削除)。
- 判定パイプライン(finalize_volatility 等)本体はまだ core に無い。self_declare の
  出力を消費する側の不変条件テストは、パイプライン移植時に本格化する。
- 既定モデル名(`gemma2`)は実在・入手性を実装時に確認(なければ広く入手可能なものに調整)。
