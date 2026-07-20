# 共有キルスイッチ + 法的姿勢の現実的再定義 — 設計スペック

> ステータス: **実装前スペック(承認待ち)**(2026-07-20)
> スコープ決定: ユーザー「設計 + キルスイッチ実装」(2026-07-20)。追加指示「UI実装にもつながるように/引数でも指定できるように」。
> 動機: 個人開発者にとって「Public 公開前に専門弁護士レビュー必須」というハードゲート([Architecture.md](../../Architecture.md) §11 / [信頼性設計メモ.md](../../信頼性設計メモ.md) §8 / [S5ノート](../../S5_Public_Phase2_法的機構先行設計.md)冒頭)は事実上クリア不能。より現実的な法的リスク姿勢=**共有をオプトイン+いつでも即オフにできる**措置を優先する。
> 出典・接続点: [S3設計ノート](../../S3_Company_Phase1_社内多ノード共有設計.md) §6(モード分離)、実コード `src/core/sync.rs`(`NodeService`・送出系経路)・`src/core/daemon.rs`(HTTP境界)・`src/core/main.rs`(CLI・起動配線)。[Roadmap.md](../../Roadmap.md) §2 S6(モード分離+UI)節と関連。

---

## 0. 本スペックが変えないもの(絶対制約)

- **S1〜S7 の段階定義・ゲート・進捗ステータス**([Roadmap.md](../../Roadmap.md) §1・§2)は不変。本作業は S6(モード分離+UI)の延長強化であり、S6 ゲート「誤操作で Private が漏れない」の趣旨を**強める**方向(=既定安全側・実行中即オフ)。段階の再定義はしない。
- **既定挙動は非破壊**: ライブラリ既定(`NodeService` 構築時)は `sharing_enabled = true`。既存の全テスト・anti-entropy ループの挙動は不変。オフは「実行中トグル」または「CLI 明示指定」でのみ発生する。
- **署名・entry_id・検証コア**には触れない。
- **Private モードの構造的不在は最強の off として維持**する。キルスイッチは company モードに runtime 制御を足すものであり、private(配送層を一切インスタンス化しない)を置き換えるものではない。両者は多層防御(private=構造的不在 / killswitch=runtime 停止)。
- **法的助言ではない**。免責([Architecture.md](../../Architecture.md) §11)を薄めない。本スペックは「software が安全な既定(private/停止)で存在・動作できるようにする」技術措置であり、Public 大規模共有運用の適法性を保証するものではない。

---

## 1. 法的姿勢の再定義(docs のみ・§4で反映)

現状 docs は「Public 公開 = 弁護士レビュー必須(ハードゲート)」。これを個人/リファレンス配布の現実に合わせて次のように**再定義**する(段階定義・ゲートは変えず、姿勢の文言のみ)。

- **既定は共有オフ(オプトイン)**: ソフトウェアは既定で「ローカル/Private」または「共有停止」で動く。ネットワーク共有はユーザーが明示的に有効化(company モード起動 + 共有 on)して初めて働く。
- **いつでも即オフ**: 共有を有効化した後も、再起動なしに即停止できる(本スペックのキルスイッチ)。停止すると announce/供出/Digest/anti-entropy/取り込みが止まり、ローカルキャッシュ利用だけが残る。
- **弁護士レビューの位置づけ**: 「software が存在・動作するためのハードゲート」ではなく、「**Public での大規模共有ネットワークを運用する主体への推奨**」へ格下げ。=個人が private/停止で使う分にはゲートに縛られない。R7(法人格分離・段階公開)とも整合(リファレンス実装は財団/OSS、運用主体が別リスクを負う)。
- この再定義は Architecture §11 / 信頼性メモ §8 の免責文言と**矛盾しない**(免責は「Public 公開前レビュー必須」を運用者への注意として残す)。docs 反映は既存文の削除でなく[補完]追記で行う。

---

## 2. キルスイッチのコア設計(`src/core`)

### 2.1 `NodeService` への追加(`sync.rs`)

```rust
use std::sync::atomic::{AtomicBool, Ordering};

pub struct NodeService
{
    // ... 既存フィールド ...
    // 共有キルスイッチ(S6強化。true=共有有効/false=即停止)。
    // 既定 true(非破壊)。private モードでは delivery=None のため意味を持たない
    // (どちらでも送出경路は構造的に不在)が、状態としては保持・報告する。
    sharing_enabled: AtomicBool,
}
```

- 構築(`new`)で `sharing_enabled: AtomicBool::new(true)` を既定とする(既存 `new` シグネチャは変えず内部初期化。テスト非破壊)。
- メソッド追加:
  - `pub fn set_sharing_enabled(&self, enabled: bool)` — `store(enabled, Ordering::SeqCst)`。トグル後の値をログ。
  - `pub fn is_sharing_enabled(&self) -> bool` — `load(Ordering::SeqCst)`。
- **判定ヘルパ**: 「送出してよいか」を1箇所に集約する内部関数 `fn sharing_active(&self) -> bool { self.delivery.is_some() && self.is_sharing_enabled() }`。既存の `self.delivery.is_some()` 判定をこれに置き換える(delivery 不在 OR 共有オフ の両方を1点で表現)。

### 2.2 ゲートする送出系5経路(`sync.rs`)

`sharing_active()` が false のとき、delivery 不在時と同じ挙動へ落とす:

| 経路 | 現状の早期リターン条件 | 変更 |
|---|---|---|
| `broadcast_announce` | `let Some(d) = &self.delivery else { return 0 }` | 先頭で `if !self.sharing_active() { return 0; }`(共有オフなら announce しない) |
| `handle_entry_request` | `self.delivery.as_ref()?` | `if !self.sharing_active() { return None; }`(供出しない) |
| `handle_digest_request` | `if self.delivery.is_none() { 空Digest }` | `if !self.sharing_active() { 空Digest を返す }` |
| `run_anti_entropy_once` | `let Some(d) = &self.delivery else { return rep }` | 先頭で `if !self.sharing_active() { return rep; }`(プルしない) |
| `handle_announce` | `let Some(d) = &self.delivery else { return NoDelivery }` | `if !self.sharing_active() { return AnnounceOutcome::NoDelivery; }`(受信プルを起動しない) |

- **ローカル機能は生存**: `ask`(検索→ミス時推論→judge→登録)・`lookup`・`get`・`entry_detail` は共有オフでも従来どおり動く。**登録時の announce だけがオフになる**(`broadcast_announce` が 0 を返す)。
- **物理データは保持**(grow-only 不変)。共有オフは「配らない/受け取りに行かない」であって、既存キャッシュの削除ではない。
- §3(d) のソース側 revocation フィルタ(同日実装)とは独立に積み上がる(両立)。

### 2.3 状態の公開(`status()`)

`svc.status()` が返す `StatusReport` に `sharing_enabled: bool`(および参考として `sharing_active: bool` = delivery あり AND enabled)を追加する。UI/CLI が現在状態を表示できるようにする。

---

## 3. 制御面: CLI 引数 + デーモンエンドポイント

### 3.1 CLI 引数(`main.rs`)— 「引数でも指定できるように」

- 新引数 `--sharing on|off`(既定 `on`。company 起動時のみ意味を持つ)。
  - `off` 指定時: `NodeService` 構築後、`svc.set_sharing_enabled(false)` を呼んでから `daemon::serve`。=「共有オフで安全に立ち上げる」。
  - private モードでは無視(元々送出経路が不在)。指定された場合は警告ログのみ。
- 起動ログに現在の共有状態を明示(`[node] sharing=on|off`)。usage 文言にも追記。

### 3.2 デーモン制御エンドポイント(`daemon.rs`)— 「UI実装にもつながるように」

UI 向け `/v1` 名前空間に追加(将来の Blazor UI がトグルボタン/状態表示に使う API 契約):

| メソッド | パス | 入出力 | 用途 |
|---|---|---|---|
| POST | `/v1/sharing` | 入 `{ "enabled": bool }` / 出 `{ "sharing_enabled": bool, "sharing_active": bool }` | 実行中トグル(再起動不要)。`spawn_blocking` で `svc.set_sharing_enabled` を呼ぶ |
| GET | `/v1/status`(既存) | 出に `sharing_enabled` / `sharing_active` を追加 | UI が現在状態を取得 |

- `/v1/sharing` は UI 向けルータ(`ui_router`)に置く(private でもマウントされる。private では常に `sharing_active=false` を返すだけで無害)。
- wire ルータ(`/wire/*`)は変更しない。
- **UI 本体(Blazor)は本スペックでは作らない**(S6 未着手のまま)。あくまで UI が消費できる HTTP API 契約を core 側に用意するに留める。将来 `blazor-ui-dev` がこの契約に対して実装する。

---

## 4. テスト仕様(`src/core/tests/`)

**配置**: 新規 `test_sharing_killswitch.rs`(`lib.rs` の `#[cfg(test)] mod tests` に `#[path]` 登録)。既存の sync テストのノード構成ヘルパーを再利用。

| # | テスト | 内容 |
|---|---|---|
| 1 | `default_sharing_enabled` | 構築直後 `is_sharing_enabled()==true`(既定非破壊の固定) |
| 2 | `killswitch_stops_serving` | company ノードで shareable エントリを1件登録 → `set_sharing_enabled(false)` → `handle_entry_request` が `None`、`handle_digest_request` が空 |
| 3 | `killswitch_stops_anti_entropy` | 共有オフ時 `run_anti_entropy_once` がプルせず即 return(peers_total 増えない/pulled==0) |
| 4 | `killswitch_stops_announce_ingest` | 共有オフ時 `handle_announce` が `NoDelivery` を返す(プル起動しない) |
| 5 | `local_search_survives_killswitch` | 共有オフでも登録済みエントリが `lookup`/`get` でヒットする(ローカル機能生存) |
| 6 | `resume_sharing` | オフ→`set_sharing_enabled(true)` で供出・Digest が復帰(トグル可逆) |
| 7 | `register_skips_announce_when_off` | 共有オフ時に `ask`/登録しても `broadcast_announce` の送信数が 0(announce 抑止)。既存の announce 計測フックがあれば利用、なければ delivery のスパイ transport で確認 |

**完了条件**: `cargo test --workspace`(default / `--features ed25519` 両ビルド)全緑。既存テスト数 + §3(d) の +5 + 本件 +7 が緑。

---

## 5. 実装体制・順序(CLAUDE.md 準拠)

- 実装エージェント = **model=fable**(メモリ確定フィードバック)。
- **順序**: §3(d)(sync.rs 既に完了)→ その脅威レビュー/docs 反映(別スペック、進行中)→ **本キルスイッチ**。sync.rs を同時編集しないよう、キルスイッチは §3(d) 系の作業完了後に着手する(project-leader が順序化)。
- 担当:
  1. `poc-core-dev`(fable): §2(sync.rs)+§3.1(main.rs)+§3.2(daemon.rs)+§4(テスト)。両ビルド緑を確認し passed 件数を報告。
  2. `threat-model-reviewer`: 「共有オフが本当に全送出経路を塞ぐか(抜け経路がないか)」「private との多層防御が崩れていないか」「トグルの並行性(AtomicBool/SeqCst)」「オフ時にローカルデータ漏洩経路が残らないか」を重点レビュー。
  3. `design-docs-editor`: §1 の法的姿勢再定義を Architecture §11 [補完] / 信頼性メモ §8 / S5ノート へ追記。Roadmap §2 S6 節へ「共有キルスイッチ(CLI `--sharing` + `/v1/sharing` + status)を実装。既定安全側・実行中即オフ。UI 消費用 API 契約を core に用意(UI 本体は S6 継続)」の横断注記(既存注記の型:「S1〜S7 の段階定義・ゲート・進捗ステータスを変更しない」を明記)。
- `cargo fmt` を brace 崩し目的でかけない(Allman 手動維持)。

---

## 6. 完了条件(このスペックの Done)

- [ ] core: `sharing_enabled` + 5経路ゲート + `status()` 反映。
- [ ] CLI: `--sharing on|off`。
- [ ] daemon: `POST /v1/sharing` + `/v1/status` 拡張。
- [ ] テスト7件、両ビルドで `cargo test --workspace` 全緑。
- [ ] 脅威レビュー: 送出経路の網羅性・多層防御維持にブロッカーなし。
- [ ] docs: 法的姿勢再定義 + Roadmap S6 横断注記(全ゲート/ステータス不変)。

---

## 7. 非目標(やらないこと)

- Blazor UI 本体の実装(S6 継続。今回は API 契約のみ)。
- Private モードの構造的不在の置き換え(維持=多層防御)。
- 共有データの purge/退避機能(将来検討。今回は「配らない/取りに行かない」まで。物理削除は grow-only 原則と別途整理が要るため範囲外)。
- ライブラリ既定を off にする変更(テスト非破壊のため既定 on。安全側の既定は「CLI/運用の選択」で担保)。
- S5 本体(Revoke 伝播・tombstone 同期等)の実装(Phase2)。
