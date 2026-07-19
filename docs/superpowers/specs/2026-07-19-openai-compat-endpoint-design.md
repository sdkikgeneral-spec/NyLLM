# OpenAI互換エンドポイント設計 — NyLLMをVS Codeカスタムエンドポイント化する

- 作成日: 2026-07-19
- ステータス: 設計完了・**棚上げ(実装保留)**。オーナー判断(2026-07-19)により、ロードマップ本筋の S4(層1内在信頼度)を優先し、本アダプタは着手しない。手戻り回避のため S4 が一区切りついた後(または VS Code から触りたい実需が出た時点)に Private モード前提で差し込む。着手時は承認後 project-leader 経由で poc-core-dev 等へ割り当て。
  - 補足: S4 未完了は本設計の技術的障害ではない(Private モードは共有が起きず信頼層 S4 と無関係、Company 共有は S3 で既に S4 なしに成立)。棚上げは純粋に優先順位の判断。
- レビュー: 2026-07-19 fable による技術レビュー実施済み。Blocker(B-1)/High(H-1〜H-3)/Medium(M-1〜M-5)/Low(L-1〜L-3)/Nit の指摘を本改訂に反映(§末尾「レビュー反映履歴」)。
- 前提ドキュメント: [Architecture.md](../../Architecture.md) / [Roadmap.md](../../Roadmap.md) / [2026-07-18-selectable-inference-backend-design.md](./2026-07-18-selectable-inference-backend-design.md)
- 関連段階: S3(daemon/sync)・S6(モード分離)を横断するデーモンAPI拡張。**新ステージは追加せず、S1〜S7の段階定義・ゲート・進捗ステータスは変更しない**(Agent層のOllama対応と同じ位置づけ)。

---

## 1. 背景と目的

### 発端
VS Code純正の「Agents window」および Copilot Chat には **BYOK(Bring Your Own Key)** 機能があり、「カスタムエンドポイント(Chat Completions / Responses / Messages のいずれかのワイヤプロトコルを喋るHTTPエンドポイント)」を言語モデルとして登録できる。ここに NyLLM を登録できれば、VS Code のチャット/エージェントの各リクエストが **NyLLM を透過的に通り、共有セマンティックキャッシュ → ミス時のみ実推論** という本プロジェクトの中核ループを、既存の開発フロントエンド上でそのまま体験できる。

### ゴール(能力の証明)
NyLLMデーモンが **OpenAI Chat Completions 互換の `POST /v1/chat/completions`(SSEストリーミング対応)** を公開し、VS CodeのBYOKに `http://127.0.0.1:<port>/v1/chat/completions` を「カスタムエンドポイント(Chat Completions)」として登録 → エージェントウィンドウ/チャットで**実際に応答が返る**ことを到達点とする。

**スコープの明確化(オーナー確認済み)**: 本作業の主眼は「**エージェントウィンドウで使える(=登録でき、動く)という能力の証明**」であって、「常用する」「社内共有を本運用する」ことを前提としない。"使える"と"使う"は別問題。したがってアダプタは**モード非依存**に作る。

### 非ゴール
- 常用ワークフローとしての作り込み・UI整備。
- 推論先の動的選択機構(将来課題。→ §8)。
- tools/function calling の実行、生成パラメータ(temperature 等)の厳密反映、Messages/Responses プロトコル対応。
  - **含意(M-5)**: `tools` は受理して無視する。その帰結として **VS Code のエージェント(ツール呼び出し)モードは機能せず、素のチャット応答挙動になる**。能力証明のスコープは「チャット応答が返ること」まで。

---

## 2. 現状の到達点(裏取り済み・fableレビューで実コード一致を確認)

実装済みで**再利用する**もの:

- `src/core/daemon.rs`: axum デーモン(`feature = "http"`)。UI向け `POST /v1/ask {question}`(daemon.rs:56-73)が既にあり、内部で `NodeService::ask()` を `spawn_blocking` 経由で呼ぶ。**UIルータと wireルータは同一リスナで供する**(daemon.rs:164-184)。company モードのみ `/wire/*` をマウント(§6 モード分離)。Agentエラーのステータスマップは `ask_handler`(daemon.rs:67-71): Timeout→504 / 他→502。
- `src/core/sync.rs`: `NodeService::ask(&self, question) -> Result<AskResult, AgentError>`(sync.rs:288-359)。
  - **ヒット**: `cache.lookup_filtered` → `render_cached_answer(e)`(`sync.rs` のプライベート関数, 約 sync.rs:620)を返す(S2.5は回答平文を保存しないため facts から合成)。Agentは呼ばない。
  - **ミス**: `agent.ask(question)?`(sync.rs:314 の `?` 早期リターン=失敗時は以降を実行しない) → `judge_entry`(pipeline) → `cache.register(...)`(sync.rs:322-331) → shareable なら `broadcast_announce`。
  - `AskResult { hit, answer, entry_id, similarity, shareable, tier, announced_to }`。
  - ロック規律: 検索・登録のみロック内、`on_token` 相当処理・announce はロック外。
- `src/core/agent.rs`: `trait Agent: Send + Sync { name; ask(&self,q)->Result<String,AgentError>; self_declare(...) }`(agent.rs:71-79。同期)。`AgentError { Unreachable, Http{status,body}, Timeout, Parse }`。`create_agent(&AgentConfig)`(agent.rs:302)。既定 `MockAgent`。`Arc<dyn Agent>`/`Box<dyn Agent>` で共有。
- `src/core/agent/ollama_agent.rs`(`feature = "ollama"`): `build_chat_request(model, content)` は **`messages:[{role:"user",content}]` の1メッセージ固定**(ollama_agent.rs:24-31)。`POST {endpoint}/api/chat` を **`stream:false`** で呼び `.message.content` を取り出す(`ureq` 同期, ollama_agent.rs:140-164)。`OllamaAgent::new` は `AgentBuilder::new().timeout(timeout_secs)`(**overall timeout**, ollama_agent.rs:126-128)。`map_transport_error` は `ureq::Error::Transport`(リクエスト開始時)専用(ollama_agent.rs:170-189)。`self_declare` は **2回目の推論呼び出し**を行う(ollama_agent.rs:208-212)。
- `src/core/cache.rs`: `register`/`lookup_filtered`/受信側再検証(cache.rs:500-)。

**足りないのは「VS Codeが理解できる皮(OpenAI互換アダプタ)」だけ**。コアロジックは既存 `NodeService` をそのまま使う。

---

## 3. アーキテクチャ

```
VS Code (BYOK, Chat Completions)
   │  POST /v1/chat/completions  (stream=true, SSE / stream=false も対応)
   │  GET  /v1/models            (最小: ["nyllm"] 1件。登録UX用。L-1)
   ▼
[新規] openai_compat アダプタ (src/core/openai_compat.rs + daemon.rs にルート追加)
   │  ・messages 正規化(content の parts配列→text連結。M-5)
   │  ・単ターン/多ターン判定(§5)、APIキー検証(§7)、SSEフレーミング(§6)
   ▼
NodeService::chat_streaming(messages, cache_eligible, on_token)  ← 新規
   │  ・cache_eligible(単ターン)且つヒット: 完成回答を on_token に分割送出(即時)。Agent非呼出。
   │  ・cache_eligible 且つミス: Agentのストリーミングを中継しつつ全文蓄積 → judge → register → (company時)announce。
   │  ・非適格(多ターン): 検索・登録をバイパスし Agentのストリーミングを中継のみ(登録しない)。
   ▼
Agent::chat_streaming(&[ChatMessage], on_token)  ← trait に追加(既定実装は最終userで ask を1チャンク送出)
   │
   ▼
OllamaAgent: 受け取った messages 列を /api/chat の messages にそのまま写し stream:true で中継
```

**責務分離**:
- `openai_compat.rs` = HTTP境界の翻訳(OpenAIスキーマ⇄内部呼び出し、SSEフレーミング)。ビジネスロジックを持たない。
- `NodeService` = 検索/登録/共有ゲート/announce(不変。ストリーミング用の新メソッドを追加するのみ)。
- `Agent` = 推論(チャット列ストリーミング経路を追加)。

**触るファイル**:
1. `src/core/daemon.rs` — `ui_router` に `/v1/chat/completions`・`/v1/models` ルート追加、SSEブリッジ。
2. `src/core/openai_compat.rs`(新規) — リクエスト/レスポンス型、messages正規化・単/多ターン判定、SSEフレーミング(純関数中心でテスト可能に分離)。
3. `src/core/agent.rs` — `Agent` trait に `chat_streaming` を追加(既定実装付き)、`ChatMessage` 型追加。
4. `src/core/agent/ollama_agent.rs` — チャット列を受ける `stream:true` 中継。
5. `src/core/sync.rs` — `NodeService::chat_streaming` 追加。
6. **`Cargo.toml`(ルート)/ `src/core` の依存(M-3)** — `tokio` に `sync` feature を追加(現状 `rt-multi-thread, macros, net, time` のみ, Cargo.toml:41)。`ReceiverStream` を使うなら `tokio-stream` を新規追加(ライセンス確認は §11-4 と同手順)。追加は `feature = "http"` の依存に閉じる。

---

## 4. リクエスト/レスポンス仕様(Chat Completions サブセット)

### リクエスト(受理する範囲)
```jsonc
{
  "model": "nyllm",               // 受理するが当面は単一backendにマップ(§8)
  "messages": [
    { "role": "system", "content": "..." },              // 任意。単ターン適格経路では破棄(H-1)
    { "role": "user",   "content": "日本の首都は?" }       // content は string | parts配列(M-5)
  ],
  "stream": true                  // true/false 両対応。既定は false 扱い
  // temperature / tools 等その他フィールドは受理して無視(エラーにしない)
}
```
- **content 正規化(M-5)**: `content` が parts 配列の場合は `type:"text"` の text を連結して文字列化する。
- `messages` 空、または(system/parts正規化後に)最終 role が user でない場合は `400`(OpenAI互換の error JSON)。
- `tool` / `assistant`(tool_calls付き)等の role を含む会話は**多ターン扱い=素通し**(§5)。

### レスポンス(非ストリーミング, `stream:false`)
```jsonc
{
  "id": "chatcmpl-<entry_id先頭 or フォールバックID>",   // 素通し時は非entry由来ID(L-2)
  "object": "chat.completion",
  "created": <unix秒>,
  "model": "nyllm",
  "choices": [{
    "index": 0,
    "message": { "role": "assistant", "content": "<回答全文>" },
    "finish_reason": "stop"
  }]
  // usage は当面省略(VS Code登録・応答表示には不要)
}
```

### レスポンス(ストリーミング, `stream:true`, SSE)
`Content-Type: text/event-stream`。OpenAI の `chat.completion.chunk` 系列:
1. 先頭チャンク: `delta: { "role": "assistant" }`
2. 本文チャンク列: `delta: { "content": "<トークン片>" }` を逐次
3. 終端チャンク: `delta: {}`, `finish_reason: "stop"`
4. `data: [DONE]`

各行は `data: <json>\n\n`。

**エラー時の2分岐(M-1)**:
- **ストリーム未開始**(先頭イベントが失敗): HTTPステータスで返す(既存 `ask_handler` と同一マップ。Timeout→504 / 他→502)。ヒット判定・Ollama接続失敗/未pull/接続タイムアウトは**すべて先頭イベントで判明**するため、この分岐で捕捉できる。
- **ストリーム開始後**(200送出後にステータス変更不可): OpenAI互換の error チャンク(`data: {"error": {...}}`)を送って `[DONE]` で閉じる。
  - **Nit**: この error チャンク形式はOpenAI公式仕様には無い事実上のプロキシ慣行であり、VS Code が表示せず単に打ち切る可能性がある(期待値として記録)。

**cache由来メタ情報**(hit/miss, similarity, entry_id, shareable, tier)の露出はMVPでは必須にしない。任意で `x-nyllm-*` ヘッダ(非ストリーミング時)や先頭SSEコメント行で観測可能にするのは nice-to-have(将来の穴埋め口)。

---

## 5. messages → 内部呼び出しの写像(セマンティクスの核)

**採用: 単ターンのみキャッシュ適格**。

### 適格判定
- `messages` から role=system を除いた列が **user 1件のみ**なら「単ターン=キャッシュ適格」。
- それ以外(過去 user/assistant/tool を含む多ターン、未知ロール混在)は「非適格=素通し」。
- `question` = その user メッセージの正規化済み content(§4 の parts連結後)。

### system プロンプトの扱い(H-1・曖昧さ排除)
- **単ターン・キャッシュ適格経路では system を破棄し、user の content のみを Agent に渡す。** 登録エントリの文脈独立性・再現性を優先する(既存 `NodeService::ask` が question のみで推論・登録するのと同一セマンティクス。system依存で生成された回答を question 単独キーで登録・announce すると共有ゲートの文脈独立性=「疑わしきは共有しない」を壊すため)。
- system を推論に含めたい要求は、本設計では**扱わない**(含めたい場合は将来、キャッシュ非適格の素通しとして別途検討)。

### 内部呼び出しの分岐
- **単ターン(適格)** → `NodeService::chat_streaming(messages=[user(question)], cache_eligible=true, on_token)`。検索→ヒット/ミス、ミス時登録あり。
- **多ターン(非適格)** → `NodeService::chat_streaming(messages=<全列>, cache_eligible=false, on_token)`。検索・登録をバイパスし、Agentのストリーミングを中継のみ(登録しない)。
  - `cache_eligible=false` 時の `AskResult` 規約(L-2): `hit=false`, `entry_id=""`, `similarity=0.0`, `shareable=false` 等のプレースホルダを返す。HTTPレスポンスの `id` は entry 由来でなく別生成(タイムスタンプ等。`created` と同様、`Date` 直接依存に注意=呼び出し側で採番)。

**根拠**: NyLLMのキャッシュは単一 question をキーにする設計であり、多ターンの文脈依存質問(例:「それを修正して」)に文脈無視のキャッシュ回答を返すと**誤ヒット**になる。単ターン限定は共有ゲート思想と一致する安全側の選択。VS Codeのコーディングチャットは本質的に多ターン・文脈依存が多いため、大半はこの**素通し経路(=文脈を保ったまま実推論に中継)**を通る前提を明記する。キャッシュの旨味は文脈独立な単発事実質問で出る。

---

## 6. ストリーミング実装

### Agent trait 拡張(B-1: チャット列を受ける)
```rust
// チャットメッセージ(OpenAI role/content の内部表現)。
pub struct ChatMessage { pub role: String, pub content: String }

// 既存の ask はそのまま残す(後方互換)。チャット列ストリーミング経路を1本追加する。
// messages: 素通し時は全列(文脈保持)、単ターン時は [user(question)] 1件。
// on_token: 到着したテキスト片ごとに呼ばれる。戻り値: 蓄積した全文(終端で judge/登録に使う)。
// 同期のまま(async を波及させない)。既定実装は最終userメッセージのみで ask を1チャンク送出。
fn chat_streaming(
    &self,
    messages: &[ChatMessage],
    on_token: &mut dyn FnMut(&str),
) -> Result<String, AgentError>
{
    let last_user = messages.iter().rev().find(|m| m.role == "user")
        .map(|m| m.content.as_str()).unwrap_or("");
    let full = self.ask(last_user)?;
    on_token(&full);
    Ok(full)
}
```
- `MockAgent` は既定実装のまま(回答全文を1チャンク送出)。テストはこれで決定的に書ける。**object-safety を保ち `Arc<dyn Agent>`/`Box<dyn Agent>` の既存利用を壊さない**(fable確認済み)。
- 単ターン経路の利便のため `ask_streaming(question, on_token)` を `chat_streaming(&[user(question)], on_token)` の薄いラッパとして併設してもよい(任意)。
- `OllamaAgent` は override: 受け取った `messages` を **そのまま** `/api/chat` の `messages` に写し(build_chat_request を列対応に拡張)、**`stream:true`** で呼ぶ。レスポンスは NDJSON(改行区切りJSON)なので1行ずつ読み、各行の `.message.content` を `on_token` に渡しつつ内部バッファへ連結。`done:true` 行で終了(その行の content は空のことが多く連結しても無害)。全文を返す。
  - **タイムアウト(H-2)**: ストリーミング呼び出しでは overall `timeout` を使わない(60秒超の生成が途中切断されるため)。`timeout_connect`(接続)+読み取り単位タイムアウト(`timeout_read` 相当)で構成する。**既存の非ストリーミング `chat()` の Agent 設定は変更しない**(別 `ureq::Agent` を持つか、リクエスト単位で設定)。
  - **エラーマッピング(H-3)**: 接続確立・非2xx(`ureq::Error::Status`)・接続時トランスポートエラーは既存 `map_transport_error` を再利用。**NDJSON読取ループ中に起きる `std::io::Error` は新規にマップする**(`ErrorKind::TimedOut`/`WouldBlock` → `Timeout`、その他 → `Unreachable` または `Parse`)。

### NodeService 拡張
```rust
// cache_eligible=true: 検索→ヒットなら完成回答を on_token に分割送出(Agent非呼出)、
//                       ミスなら agent.chat_streaming で中継+全文蓄積→judge→register→announce。
// cache_eligible=false: 検索・登録をバイパスし agent.chat_streaming の中継のみ(登録しない)。
pub fn chat_streaming(
    &self,
    messages: &[ChatMessage],
    cache_eligible: bool,
    on_token: &mut dyn FnMut(&str),
) -> Result<AskResult, AgentError>
```
- **ヒット経路**(cache_eligible且つ lookup 命中): `render_cached_answer(e)` を得て、**`char` 境界で N 文字単位**(UTF-8バイト分割禁止, L-3)に分割し `on_token` に流す。`hit:true` の `AskResult` を返す。Agentは呼ばない。
- **ミス経路**(cache_eligible且つ非命中): 既存 `ask` のミス分岐と同一の後処理(`judge_entry` → `register` → shareable なら `broadcast_announce`)を、`agent.chat_streaming(...)?` で得た全文に対して行う。**登録は全文確定後**。`?` 早期リターンで Agent失敗時は登録しない不変条件を維持(sync.rs:314 と同構造)。
- **素通し経路**(cache_eligible=false): `agent.chat_streaming(messages, on_token)?` で中継のみ。登録・announce・judge を行わず、プレースホルダ `AskResult`(L-2)を返す。
- ロック規律は既存 `ask` を踏襲(検索・登録はロック内、`on_token` 実行中と announce はロック外)。

### HTTP境界(daemon.rs)—先頭イベント peek 方式(M-1/M-2)
- `NodeService::chat_streaming` は同期(`ureq`/`reqwest::blocking`)なので、`spawn_blocking` + `tokio::sync::mpsc` チャネルで橋渡しする。チャネルのメッセージは:
  ```rust
  enum Ev { Token(String), Done(Box<AskResult>), Fail(AgentError) }
  ```
- **`on_token` クロージャは spawn_blocking の `'static + Send` move クロージャ内部で構築**し、`Arc<NodeService>` と `Sender` を所有させ、`sender.blocking_send(Ev::Token(..))` で送る(`&mut dyn FnMut` 自体は Send 不要=同一スレッド内呼び出し)。**注意: `blocking_send` はランタイムスレッドから呼ぶと panic するが、spawn_blocking 内なら安全**。
- **ハンドラは最初の1イベントを await してから分岐(peek)**:
  - 先頭が `Fail` → HTTPステータス(504/502)を返す(SSE開始しない)。
  - 先頭が `Token`/`Done` → その1件を先頭に据えた SSE ストリーム(`text/event-stream`)を構成し、残りのイベントを流す。
- ストリーム完了(Agentの `done:true`)を受けた時点で finish_reason=stop チャンク + `[DONE]` を送出し、**judge/register/announce はその後に同 blocking タスク内で継続実行**する(M-2: VS Code上の表示完了後に接続を長く開いたままにしない。judge以降は全文=Ok確定後にのみ到達するため「Agent失敗時は登録しない」に影響なし)。
- `stream:false` の場合は全 `Ev` を集約し、`Fail` があればステータス、なければ `chat.completion` JSON を1レスポンスで返す。

---

## 7. 設定・セキュリティ(最小構成)

- `feature = "http"` のデーモンに相乗り(ストリーミング実体は `feature = "ollama"` 経路で有効)。
- **既定は 127.0.0.1 ループバック限定バインド**。同一マシン外からは到達不能。
- **APIキー**: 環境変数(例 `NYLLM_HTTP_API_KEY`)に共有シークレットを設定した場合のみ `Authorization: Bearer <key>` を検証し、不一致は `401`。未設定ならダミー受理(ローカル前提)。
- **非ループバックバインド時の露出(M-4)**: `serve()` は UI/wire を**同一リスナ**で供する(daemon.rs:164-184)ため、company 多ノードでピアが `/wire/*` に到達するよう非ループバックにバインドすると、`/v1/chat/completions`(と既存 `/v1/ask`)も LAN に露出する。よって **非ループバックにバインドする場合は `NYLLM_HTTP_API_KEY` の設定を必須**とする(未設定+非ループバックなら起動時に警告し、`/v1/chat/completions` を401固定)。UI/wire のリスナ分離は将来課題として注記。
- **モード非依存**: `--mode company|private` どちらの起動でも `/v1/chat/completions` は動く。既定デモは company(単ターン事実質問が社内共有され得る)。private では配送層が無くローカルキャッシュのみ(§6 モード分離の既存挙動をそのまま利用)。

---

## 8. 推論先の動的選択(将来課題・スコープ外)

- 今回は既定 backend 固定(`AgentConfig::from_env()` に従う)。`model` フィールドは受理するが当面は単一 backend にマップする。
- 「後々、推論先を選べるように」(オーナー要望)は、`model` フィールド → backend/モデルのルーティングとして将来差し込む。今回は**差し込み口を意識した構造**(model を無視せず握っておく)に留め、実装はしない。

---

## 9. テスト計画(CLAUDE.md: 実装と同時にテスト)

新規 `src/tests/test_openai_compat.rs`(純関数中心で決定的に):
- content 正規化: string / parts配列(text連結) / 混在。
- messages→内部写像: 単ターン/多ターン判定(system除外後の user 数、assistant/tool 混在は多ターン)、最新 user 抽出、空・非user終端の 400。system が単ターン経路で破棄されること。
- 非ストリーミング整形: `AskResult` → `chat.completion` JSON(role/content/finish_reason)。素通し時の非entry由来 `id`。
- SSEフレーム系列: 先頭 role delta → content delta 列 → 終端 finish_reason=stop → `[DONE]`。分割が全文を正しく再構成すること(char境界)。
- 先頭イベント peek 分岐: `Fail` 先頭 → ステータス(504/502)、`Token`/`Done` 先頭 → SSE。
- ヒット経路(MockAgentでキャッシュ登録済み→再質問でhit、Agent非呼出)。
- ミス経路(登録が起き、shareable判定・provenanceが従来 `ask` と一致)。
- 多ターン素通し(検索・登録が起きないこと、全列がAgentへ渡ること)。
- Agent失敗: 先頭(502/504)/開始後(errorチャンク)。`OllamaAgent` のストリーム行パース純関数(NDJSON1行→content抽出、done検知)、読取ループ中 io::Error のマッピング。

回帰:
- 既存 `cargo test --workspace` を **default / `--features ed25519` の両ビルドで緑維持**。
- ストリーミング実体テストは `feature = "http"` / `"ollama"` 前提部分を feature ゲートで分離し、既定ビルドを重くしない。
- Allman ブレース手動維持(`cargo fmt` を無差別実行しない)。

**能力の証明(手動E2E)**: 実際に VS Code BYOK に `http://127.0.0.1:<port>/v1/chat/completions` を「カスタムエンドポイント(Chat Completions)」として登録(モデルIDは `nyllm` を手入力、または最小 `/v1/models` で自動列挙・L-1)し、単ターン質問で応答が返る/2回目でヒットが効く/多ターンで文脈を保った応答が返ることを手動確認。これが本設計の到達点。

---

## 10. 不変条件(壊さないこと)

- S2.5エントリ形式(`entry_id`/署名/`core_bytes` 正準化)・ロード時 verify・共有ゲート・受信側再判定の不変条件は一切変えない。本設計は `NodeService`/`cache` の登録・検索ロジックを**呼ぶだけ**で、そのセマンティクスを変更しない(fable確認: register/lookup_filtered/verify_envelope に触れない)。
- 「Agent失敗時はエントリを登録しない」(設計 2026-07-18 §4)をストリーミング経路でも維持(judge以降は全文Ok確定後にのみ到達)。
- モード分離(private は配送層を持たない)の既存挙動を変えない(M-4 の露出面の運用注記を除く)。
- 段階定義・ゲート・進捗ステータス(Roadmap §1・§2)は変更しない(新ステージではなくデーモンAPI拡張)。

---

## 11. 実装順序(概略。詳細プランは writing-plans で別途)

1. `openai_compat.rs` の純関数(型・content正規化・messages判定・SSEフレーミング)+ 単体テスト。
2. `Agent::chat_streaming`(既定実装)+ `ChatMessage` + `NodeService::chat_streaming`(ヒット/ミス/素通し)+ テスト(MockAgent で決定的)。
3. `Cargo.toml` に tokio `sync` feature(必要なら tokio-stream。ライセンス確認)。
4. `daemon.rs` に `/v1/chat/completions`・`/v1/models`(spawn_blocking + mpsc + 先頭イベント peek → SSE、非ストリーミング分岐、APIキー検証、非ループバック時の必須キー)。
5. `OllamaAgent::chat_streaming`(列対応 build_chat_request、`stream:true` NDJSON 中継、ストリーミング用タイムアウト、読取ループ io::Error マッピング)+ 純関数テスト。
6. 手動E2E(VS Code BYOK 登録)で能力を確認。
7. Roadmap §2 に横断注記(Agent層Ollama対応と同様、段階定義は不変)を追記。

---

## レビュー反映履歴

- 2026-07-19 fable レビュー反映:
  - **B-1**(Blocker): 多ターン素通しに必要なチャット列APIが無い → `Agent::chat_streaming(&[ChatMessage],...)` を主APIに(案A採用)。`OllamaAgent` は messages 列をそのまま `/api/chat` に写す。§3/§5/§6。
  - **H-1**: system の行き先未定義 → 単ターン適格経路では system 破棄・user content のみ推論/登録。§5。
  - **H-2**: ureq overall timeout がストリームを途中切断 → 接続+読取単位タイムアウトに分離、非ストリーミング設定は不変。§6。
  - **H-3**: `map_transport_error` 再利用範囲 → 接続/非2xx は再利用、読取ループ中 io::Error は新規マップ。§6。
  - **M-1**: 未開始/開始後のエラー2分岐 → `enum Ev` + 先頭イベント peek。§4/§6。
  - **M-2**: finish/[DONE] を done受領時に送り、judge/register/announce はその後継続。§6。
  - **M-3**: 依存変更(tokio `sync` feature / tokio-stream)を触るファイルに追加。§3/§11。
  - **M-4**: 非ループバックバインド時の UI/wire 同一リスナ露出 → 非ループバックでは APIキー必須。§7。
  - **M-5**: content parts配列の連結、未知ロールは素通し、tools無視=エージェント(ツール)モードは非機能。§1/§4/§5。
  - **L-1**: 最小 `/v1/models`(またはモデルID手入力)。§3/§9。 **L-2**: 素通し時の `AskResult`/`id` プレースホルダ規約。§4/§5。 **L-3**: ヒット分割は char 境界 N 文字。§6。
  - **Nit**: error チャンクは非公式慣行で VS Code が無視しうる旨、`render_cached_answer` は sync.rs のプライベート関数である旨を明記。§4/§2。
  - fable が「問題なし」と確認: spawn_blocking+mpsc ブリッジの Send/ライフタイム健全性、trait 既定実装の object-safety、不変条件(register/lookup/verify 非改変・Agent失敗時非登録・モード分離)、単ターン判定の安全側倒し、ロック規律の整合。
