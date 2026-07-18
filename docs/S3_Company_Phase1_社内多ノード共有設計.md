# S3(Company Phase1)社内多ノード共有設計 — レジストリ発見 + ノード間直接配送

> ステータス: **確定設計・実装完了(Company Phase1縮約範囲)**(2026-07-17設計確定 / 2026-07-18実装完了。`src/core/`〔`node.rs`/`transport.rs`/`wire.rs`/`sync.rs`/`registry_client.rs`/`daemon.rs`/`policy.rs`/`main.rs`〕+ 別クレート `registry` に反映済み。`cargo test --workspace` が default/`--features ed25519` の両ビルドで全緑〔S3ゲート判定テスト `s3_gate_two_or_more_nodes_share` を含む〕、脅威モデルレビュー(2026-07-18)実施済み〔判定=承認、修正必須なし〕。実装後の実装記録・既知制約・Phase2申し送りは §7・§8・§10 を参照。進捗の一次情報は引き続き [Roadmap.md](./Roadmap.md) §0[C]・§1・§2「S3 P2P化」節)
> 出典: [S2.5_エントリ形式設計.md](./S2.5_エントリ形式設計.md) §6「保存レイアウトとload/verify手順」・§13「実装後メモ/既知の妥協点」、[Architecture.md](./Architecture.md) §6「キャッシュエントリ データモデル」/ §9「ネットワークモード分離設計」、[信頼性設計メモ.md](./信頼性設計メモ.md) §9「主戦場の段階展開」、[設計レビュー_2026-07.md](./設計レビュー_2026-07.md) §3「セキュリティ脅威モデル(敵対的棚卸し)」/ §4.7「主戦場の決定」、[Roadmap.md](./Roadmap.md) §0「主戦場の段階展開」
> 前提ドキュメント: [Architecture.md](./Architecture.md) / [信頼性設計メモ.md](./信頼性設計メモ.md) / [設計レビュー_2026-07.md](./設計レビュー_2026-07.md) / [S2.5_エントリ形式設計.md](./S2.5_エントリ形式設計.md) / [Roadmap.md](./Roadmap.md)
> 対象実装(実装済み): `src/core/`(Rust)新規モジュール群(`node.rs`/`transport.rs`/`wire.rs`/`sync.rs`/`registry_client.rs`/`daemon.rs`/`policy.rs`/`main.rs`)+ `poc/` からの移植分(§9参照)。別クレート `registry`(発見専用の軽量 `axum` サービス)。`poc/src/` は本ノートの対象外(引き続き凍結・参照実装)
> 作成日: 2026-07-17

---

## 本ノートの位置づけ

本ノートは **S3(P2P化)を Company Phase1 に縮約した本体設計** であり、段階展開([信頼性設計メモ.md](./信頼性設計メモ.md) §9「主戦場の段階展開」/ [設計レビュー_2026-07.md](./設計レビュー_2026-07.md) §4.7「主戦場の決定」)が定める「**[C] `src/core`(Rust)への昇格 = Company Phase1本体**」([Roadmap.md](./Roadmap.md) §0 更新シーケンス [C])のうち、多ノード共有部分を実装可能な粒度まで具体化したものである。

- **確定設計であること**: 以下の §0〜§11 は、既に採用決定済みの戦略(前掲 §4.7・§9)を前提に、fable が具体化した Company Phase1 の S3 確定仕様である。§11「要判断点」に限り、複数の選択肢が存在した論点として fable 推奨案を既定採用とし、各項目に「採用(推奨)/代替」を明記する形で残す。
- **実装は別段階**: 実装は本ノート確定・承認後の別段階として、project-leader 経由で poc-core-dev / blazor-ui-dev に割り当てられる(+テスト実装、CLAUDE.md 運用ルール規則4「実装したコンポーネントには必ずテストを実装する」)。本ノート自体は docs であり、`poc/src/` にも将来の `src/core/` にも一切コードを含まない(本文書はあくまで設計仕様の記録であり、コードそのものは含めない)。`poc/` は S1/S2 の参照実装として引き続き凍結する([Roadmap.md](./Roadmap.md) §0[C])。**追記(2026-07-18)**: 上記の別段階実装は完了した(`src/core/` + 別クレート `registry`)。全テスト緑・脅威モデルレビュー承認済み(→ §7・§8・§10の実装記録)。進捗ステータスの一次情報は [Roadmap.md](./Roadmap.md) を参照。
- **既存不変条件との関係**: CLAUDE.md が定める不変条件(`entry_id = sha256(signed_payload)`〔S2.5 で `hex(sha256(core_bytes))` へ精緻化済み〕・ロード時検証・保守的共有ゲート・しきい値 0.80/0.90)と矛盾しない。本ノートが追加するのは「ロード時検証をノード間配送の文脈で誰が・どう再実行するか」という配送層であり、S2.5 の検証パイプライン自体は一切変更しない(→ §3「受信側の検証手順」)。
- **S2.5設計との関係**: 本ノート §3「受信側の検証手順」の手順3〜9は、[S2.5_エントリ形式設計.md](./S2.5_エントリ形式設計.md) §6「保存レイアウトと load/verify 手順」の手順2〜8と同一の検証ロジックであり(手順2〔組織PKI検証〕のみが S3 新規)、単一ノードとマルチノードで検証コードパスを共有する。また S2.5 §13「実装後メモ/既知の妥協点」に記録された3件の S3 持ち越し事項(reload再導出の非単調性・未分類tierの保守側反転・版フラッディング上限)は、本ノート §4 末尾「S2.5 §13 残課題との関係」で本設計への橋渡しを行う。
- **Roadmap.mdとの関係**: 進捗管理の一次情報は引き続き [Roadmap.md](./Roadmap.md) であり、本ノートは内容(設計仕様)の一次情報として Roadmap.md §0[C]・§2「S3 P2P化」節・§5 から索引される。S3自体の「未着手/完了」というゲート判定([Roadmap.md](./Roadmap.md) §1)は本ノートの追加によって変更されない(設計成果物へのポインタが加わるのみ)。
- **範囲外**: Public Phase2固有の機構(DHT/witness/アンカー/ステーク/裁定)の詳細設計は書かない。本ノートは Company Phase1 の範囲に限定し、Phase2への非破壊性の確認のみ §8 で行う(個別機構の詳細検討は [設計レビュー_2026-07.md](./設計レビュー_2026-07.md) §4.2〜§4.5、Public Phase2で改めて判断)。

---

## 0. 全体を貫く決定

- **シビル不在を「機能を消す」ためでなく「信頼の出所を組織PKIに一本化する」ために使う。** Phase1で消えるのは Sybil耐性のための機構(PoW ID/witness多数/ステーク/評判裁定/eclipse対策/DHT)であって、説明責任のための機構(author_sig/受信側再導出/モード物理分離)は消さない。この線引きが over-engineering回避と Phase2非破壊性を同時に満たす鍵である。
- **最重要の非破壊性原則: 中央要素(レジストリ)は「発見(discovery)」だけを担い、「信頼(trust)」は一切担わせない。** 検証は各ノードが `core_bytes` に対して自律的に行う([S2.5_エントリ形式設計.md](./S2.5_エントリ形式設計.md) §6 load/verify)。この分離を守れば、Phase2でレジストリをDHT発見に差し替えても信頼レイヤは一行も変わらない。破って「レジストリにあるから信頼」を焼き込むとPhase2が作り直しになる(→ §8で再掲)。

---

## 1. ノード・信頼モデル

### 鍵とID

- `node_id = hex(sha256(author_pub))`。Ed25519公開鍵のハッシュ(ドメイン分離タグ `nyllm/node/v1`。S2.5の `nyllm/entry/v1` / `nyllm/qkey/v1` とは別タグ)。
- 鍵発行 = 組織内部CA(PKI)。各ノードは Ed25519 鍵ペアを持ち、公開鍵に組織CAが署名した `node_cert` を配布する。`node_cert = CA_sign(node_id || node_pub || 有効期限 || mode許可)`。
- `author_sig` の組織内検証は2段構成: (a) `core_bytes` に対する `author_sig` を `author_pub` で検証する([S2.5_エントリ形式設計.md](./S2.5_エントリ形式設計.md) §5・§6)、(b) その `author_pub` が有効な `node_cert` を持つ(CA署名OK・未失効)ことを確認する。この2段で「組織の正規ノードが署名した」ことを担保する。

### シビル不在で要らなくなるもの(Phase1で実装しない)

| 消すもの | 理由 |
|---|---|
| PoW ID生成 | 鍵はCA発行=偽IDを安く量産できない。限界コスト付与が不要 |
| witness多数(誕生証明) | 誕生時刻はCA発行時刻+NTP共通時計で足りる。独立多数の観測者が不要 |
| ステーク/スラッシング経済 | 悪意ノードは人事・契約・CA失効で排除できる。経済的抑止が不要 |
| 局所EigenTrust/評判裁定 | 全ノード同格に信頼する(組織信頼)。近傍伝播計算が不要 |
| eclipse対策/S-Kademlia | ピア発見が中央レジストリのため、視界を攻撃者が独占できない |
| DHT | 社内規模は全複製で足りる(→ §5)。キー空間分割が不要 |

### 消さないもの(組織信頼下でも維持)

- `author_sig` + `node_cert`: 「誰が入れたか」= 過失毒の追跡・R4説明可能性・Private混入検知の基盤。組織信頼は「悪意がない」であって「間違えない」ではないため、追跡は必須のまま残す。
- CA失効リスト(CRL): 侵害・誤設定ノードの鍵を組織CAが失効させる。Phase2の revocation(エントリ単位失効)とは別物(ノード単位PKI失効)である(→ §7)。

---

## 2. 共有トポロジ

### 推奨: 中央レジストリ(発見専用) + ノード間直接HTTP

```text
        ┌─────────────┐
        │  Registry   │  ← ピア一覧 + CA証明書配布のみ。エントリは通さない
        │ (discovery) │
        └──────┬──────┘
       登録/一覧 │
     ┌─────────┼─────────┐
     ▼         ▼         ▼
  ┌──────┐  ┌──────┐  ┌──────┐
  │Node A│◄─┤Node B│◄─┤Node C│   ← エントリ配送はノード間直接(HTTP/gRPC)
  └──────┘─►└──────┘─►└──────┘
   各ノード = ローカルデーモン(axum)+ 全複製キャッシュ + UIバックエンド
```

- レジストリの責務(最小): (1) ノード参加/離脱管理(`node_id` + エンドポイントURL + `node_cert`)、(2) ピア一覧提供、(3) CA公開鍵/CRL配布。エントリデータは一切通さない・保持しない。
- エントリ配送はノード間直接(→ §3)。レジストリはボトルネックにも単一障害の信頼点にもならない(落ちても既知ピア間の同期は継続する)。
- CLAUDE.md方針(実装本体は `src/core/` Rust + `src/ui/` Blazor、interopはローカルデーモン+HTTP/gRPC)に沿い、各ノードは「Rustコアのローカルデーモン(axum)」とし、UI(Blazor)は同一ノードのデーモンをHTTPで叩く(→ §9)。ノード間も同じHTTPサーバの別エンドポイントとし、レジストリも同型の軽量axumサービスとする。

### 代替と不採用理由

- 代替A: LAN gossip(mDNS+フルメッシュ)— レジストリ不要だが拠点跨ぎ(VPN)で発見が破綻する。**却下**: 複数拠点構成に合わない。
- 代替B: 共有ストア(NFS/オブジェクトストレージ)— 実装は最小だが「各ノードが検証して取り込む」主導権が消え、共有ストアが単一障害かつ信頼点(誰でも書ける=過失毒の温床)になる。**却下**: 受信側再導出の不変条件と噛み合わない。
- 非採用(Phase2前借り回避): DHT — 社内規模で全複製が成立する以上、キー空間分割は不要な複雑性である。Phase2で導入する。

---

## 3. エントリ配送プロトコル

### 方式: プル型を主 + 軽量announce(通知のみプッシュ)

実データを押し付けない。「新規があるよ」の通知だけを流し、実体は受信側が主導でプルする。理由は、受信側が検証してから取り込む主導権を持つこと(過失毒・誤設定ノードへの耐性)にある。

### メッセージ型(ワイヤプロトコル `nyllm-wire/v1`)

| メッセージ | 方向 | ペイロード | 用途 |
|---|---|---|---|
| Announce | push(通知) | `{entry_id, question_key, created, node_id}` | 新規登録の通知。実体は含めない |
| Request | pull | `{entry_id}` | 実体を要求 |
| Transfer | pull応答 | S2.5 `.entry` エンベロープ(`core_b64` + `author_pub` + `author_sig`) | 実体の転送。`mutable_state` は送らない |
| Digest | anti-entropy | `{entries:[(entry_id, question_key)...]}`(またはハッシュ要約) | 定期同期。取りこぼし補償 |

**要点**: Transferは immutable部分(core+署名)のみを運ぶ。`mutable_state`(`shareable`/`tier_operative`/`volatility_class_operative`等)は送らない(受信側が捨てて再導出する=帯域の無駄かつ「送信者判断を信頼しない」原則の徹底)。これは [S2.5_エントリ形式設計.md](./S2.5_エントリ形式設計.md) §2「mutable_state の全フィールド」の設計と完全に一致する。

### 受信側の検証手順(S2.5 load/verify をノード間文脈で)

Transfer 受信時:

```text
1. エンベロープをパース → core_b64, author_pub, author_sig
2. author_pub が有効な node_cert を持つか(CA署名OK + CRL未失効)   ← S3で追加(組織PKI)
3. core_bytes = base64_decode(core_b64)
4. sha256_hex(core_bytes) == announce の entry_id か?              ← ハッシュ照合(改ざん検知)
5. Signer::verify(author_pub, author_sig, core_bytes)             ← 署名検証(偽造防止)
6. parse_core(core_bytes) → ImmutableCore。失敗→drop
7. question_key を core.question_norm から再計算
8. embedding = local_embedder.encode(core.question_norm)          ← ローカル再計算
9. judge_entry 再実行 → shareable/tier_operative/volatility_class_operative を再導出  ← 送信者値を信頼しない
   └ shareable=false なら「受け取るが自ノードでは共有伝播しない」扱い(→ §7)
10. question_key 重複排除・冪等マージ(→ §4)して取り込み
```

上記のうち **手順2のみが S3 新規(組織PKI検証)** であり、手順3〜9 は [S2.5_エントリ形式設計.md](./S2.5_エントリ形式設計.md) §6「保存レイアウトと load/verify 手順」の手順2〜8(`core_bytes = base64_decode(core_b64)` → `sha256_hex` 照合 → `Signer::verify` → `parse_core` → `question_key` 再計算 → `embedding` 再計算 → `judge_entry` 再実行)とそのまま同一である(エンベロープ解析を本ノートの手順1、組織PKI検証を手順2として挿入したため、以降の手順番号がS2.5側から+1ずれている点のみが差異)。手順10(`question_key` 重複排除・冪等マージ)は S2.5 §6「重複排除・バージョニング」に対応する(→ §4)。配送は「S2.5の検証パイプラインをネット越しに呼ぶだけ」という構造であり、単一ノードとマルチノードで検証コードパスを共有する(trait駆動思想の踏襲)。

### announceの配り方(Phase1最小)

既知ピア全員へ直接announceする(社内規模のフルメッシュ通知で足りる)。gossip/フラッディングは使わない(Phase2でスケールが必要になった際に導入する)。取りこぼしは Digest の定期交換(anti-entropy)で補償するため、announceは best-effort でよい。

---

## 4. 一貫性と重複排除

### 結果整合 + 複数版併存(非目標N3 / Architecture.md §2.2・§5.2)

全順序合意は行わない。キャッシュ集合を `entry_id` をキーとした grow-only set(CRDT的)として扱う。マージ=和集合(可換・結合・冪等であり、どの順序で同期しても収束する)。

- 同一 `entry_id`: 冪等マージ(既にあればスキップ。`core_bytes` 同一なので衝突しない)。
- 同一 `question_key` × 異 `entry_id`: 別版として併存させる。検索時は複数版を候補にし、消費側が volatility/created で選ぶ(将来はtrustも加味、Phase1は created 新しい順+shareable)。
- 削除: Phase1は grow-only。ノード単位の問題はCA失効で対処し、エントリ単位の失効(revocation)はPhase2とする。TTL満了は検索から除外するが物理削除はしない(→ §7)。

### 時刻 = 組織共通時計(NTP)で足りる

- `created`(UTC RFC3339)はNTP同期の各ノードローカル時計で付与する。組織内NTPで秒精度が揃い、版の新旧比較・TTL判定には十分である。
- witnessが不要な理由: witnessは「共通時計を信頼できない敵対環境」で過去日詐称を防ぐ機構([設計レビュー_2026-07.md](./設計レビュー_2026-07.md) §4.2)である。組織内は共通NTPを信頼でき、かつ署名者がCA正規ノードであるため、過去日詐称の動機と手段が両方消える。Phase2でPublicに出た瞬間にこの前提が崩れ、witness/アンカーが必要になる(→ §8の空スロットで待機)。

### S2.5 §13 残課題との関係

[S2.5_エントリ形式設計.md](./S2.5_エントリ形式設計.md) §13「実装後メモ/既知の妥協点」に記録された3件のS3持ち越し事項は、本ノートの以下の箇所に直接対応する。

| S2.5 §13 の項目 | 本ノートでの対応箇所 | 関係 |
|---|---|---|
| High-1: reload時再導出の非単調性(§6手順8の`judge_entry`再実行により、`shareable`が false→true に反転しうる) | 本ノート §3「受信側の検証手順」手順9 | 単一ノードのreloadでは「同一ノードの同一エントリが時点によって導出結果を変える」非単調性だったが、マルチノードでは「同じ`entry_id`を保持する複数ノードが、それぞれ異なる時点で§3手順9の`judge_entry`を実行し、それぞれ独立に`shareable`/`tier_operative`を再導出する」構図になる。ノード間で`judge_entry`のロジック更新タイミングがずれると、同一entry_idについてノードAでは共有可・ノードBでは共有不可という状態が一時的に併存しうる(非単調性がノード間の時点差に転写される)。Phase1では各ノードの伝播判断がローカルのみに閉じる(自ノードが再導出した`shareable=false`は他ノードへの伝播を止めるだけで、他ノードの導出結果には影響しない)ため実害はないが、複数版併存(本節)の判断ロジックにこの時点差が反映されることは設計として認識しておく |
| Medium-1: 未分類tierの保守側反転(tierを共有ゲートに配線する段階で、既定`initial_tier=Low`の未分類エントリを`Tier-H`へ反転させる予定) | 本ノート §3「受信側の検証手順」手順9(`tier_operative`の再導出) | この反転は各ノードの受信側`judge_entry`再実行(手順9)そのものに実装される。反転前は全てのP2P受信エントリが`tier_operative=Low`側で導出されるが、反転後は`initial_tier`未分類のエントリがP2P経由でも`Tier-H`側に倒れ、§7「共有ゲート」の伝播判断に直結する。反転のタイミングはノード単位のソフトウェア更新(judge_entryのバージョン)に依存するため、上記High-1と同様にノード間で一時的な非一貫が生じうる点は共通の注意点である |
| Low-1: 版フラッディング(登録時の重複排除が`entry_id`完全一致のみで、`question_key`あたりの版数上限がない) | 本ノート §3「メッセージ型」のAnnounce/§4「結果整合+複数版併存」のgrow-only集合 | S2.5 §13は「P2P受信取り込み時は`created`が攻撃者制御になるため、`question_key`あたりの版数上限・レート制限が必要になりうる」と記録している。これは本ノートのAnnounce(best-effort・レート制限なし)とgrow-only mergeの組み合わせがそのまま実装対象であることを意味する。Phase1設計では版数上限・レート制限を明示的に実装しない(over-engineering回避の線、→ §7)ため、この残課題はPhase1でも未解消のまま残ることを明記する。組織信頼下(悪意ノードがCA管理下にある)ではリスクは限定的だが、誤設定ノードによる大量announceは運用上のノイズになりうる |

---

## 5. 検索

### 推奨: 各ノードが全複製を保持しローカル総当たり検索(S1のO(n)踏襲)

- 社内規模(想定〜10^5)ではコサイン総当たりは実測 n=10,000 で約5.4ms([Roadmap.md](./Roadmap.md) S1ベンチ)、10^5でも数十ms級でhot path許容範囲である。
- ストレージは軽量: 保存はcore(事実トリプル+メタ)のみとし、embeddingは非保存(ローカル再計算/起動時一括生成しメモリ保持)とする。10^5件でも数十MB級に収まる。
- 利点: hot pathがネットに出ない(レイテンシ最小・オフライン耐性・レジストリ障害時も検索継続)。ヒット率実測目的にも、全体を見られる方が測定が容易である。

### 代替と不採用

- 代替: ピア問い合わせ(scatter-gather)— 部分保持で検索時に全ピア問い合わせを集約する方式。ストレージは節約できるがhot pathがネット依存になりレイテンシ・可用性が悪化する。社内規模では全複製が優位。限界を超えたらPhase2でANN+部分複製へ移行する(検索traitの境界は維持する)。
- ANN化: Phase1は不要(O(n)で足りる)。検索をtrait化しておき必要時に差し替えられるようにする([S2.5_エントリ形式設計.md](./S2.5_エントリ形式設計.md) 同様、境界だけを今用意する)。

---

## 6. モード分離(Architecture.md §9 / S6相当の最小)

### 起動時分離(推論時判定でない)

```text
nyllm-node --mode company   # 共有デーモン起動・レジストリ登録・配送参加
nyllm-node --mode private   # 配送デーモンを起動しない・レジストリ未登録・ローカルのみ
```

**物理分離の実装**:

- 別 `store_dir`: `company_store/` と `private_store/` を完全に別ディレクトリとし、プロセスは片方のみをマウントする。
- Privateは配送層を起動しない: `--mode private` では `transport`/`sync`/`registry_client` を一切インスタンス化しない。announce/Transferが物理的に発行不能になる。
- レジストリ未登録: Privateノードはレジストリに現れず、他ノードのピア一覧に載らない(=Requestの宛先にならない)。

**Private混入しない保証(多層)**:

1. Privateは配送コード未起動(送信経路が構造的に不在)。
2. 仮にCompanyデーモンにPrivate質問が来ても、S2.5共有ゲート(個人参照L0+受信側再導出)で `shareable=false` になり伝播しない。
3. UIがモード別アイコンで現在モードを常時表示する(→ Architecture.md §9、実装は blazor-ui-dev 担当)。

**over-engineering回避**: Company/Privateの2モードのみとする。Publicモードは Phase2(レジストリの代わりにDHT/アンカーが要るため、起動オプションだけ予約し中身は未実装とする)。

---

## 7. Company Phase1でも要る最小の脅威対処

シビルは無いが過失・陳腐化・侵害は残る。組織信頼を前提に、守る/守らないを明示的に線引きする。

| 残存脅威 | Phase1の対処 | Phase2送り |
|---|---|---|
| 過失による毒 | shareable受信側再導出を維持(緩いエントリが入っても受信ノードのゲートで止まる)。author_sigで発生源を追跡し人手で是正する | 独立検証・自動裁定・スラッシング |
| 陳腐化(古いエントリ滞留) | volatility TTLで検索除外(slow=created+猶予、volatileは非共有)。物理削除はしない | witness安定性観測・自動再評価キュー |
| 侵害/誤設定ノード | 組織CAの証明書失効(CRL)で `node_id` 鍵を無効化し、以後author_sig検証が落ちて由来エントリは受信されない。既取込分は再検証で除外する | エントリ単位 revocation フラッディング |
| Private混入 | §6の多層防止 | (同左、Publicでも維持) |
| 幻覚(自然発生) | Tierタグ付与まで(`initial_tier`/`tier_operative`)。強制はしない | 幻覚パリティのTier-H裁定強制・外部照合 |

**over-engineeringしない線(Phase1で明示的にやらないこと)**: 独立生成一致率計算、層3抜き打ち再推論、評判スコア、ステーク、witness、アンカー、regurgitationフィルタ、エントリ単位revocation、DHT、eclipse対策、PoW。これらは空スロットのまま保留する。「組織信頼で代替できるものは代替する」を徹底する。

**CRLとPhase2 revocationの違い**: Phase1のノード失効(CA/CRL)=「鍵を無効化」する粗粒度・PKI標準の機構である。Phase2 revocation=「特定エントリを失効」させる細粒度の機構であり、権限モデルと反証が要る([設計レビュー_2026-07.md](./設計レビュー_2026-07.md) §3.6課題5)。前者は今実装し、後者はPhase2とする。両者を混同しない。

### 実装記録(2026-07-18、脅威モデルレビュー承認済み)

`src/core/` 実装(本ノート確定後の別段階)にて、脅威モデルレビュー(2026-07-18)由来で以下を上記の判断枠組みの範囲内で具体化した(新規の設計判断ではなく、既存項目の実装粒度の記録)。判定=承認(修正必須なし、production不変条件はすべて維持)。

| 項目 | 内容 | 実装箇所 | 根拠 |
|---|---|---|---|
| H-1: CRL失効著者の遡及除外 | 失効ノードの `author_pub` を検索(`is_searchable`)・供出(`handle_entry_request`)・Digest列挙(`handle_digest_request`)の3経路すべてで即時除外する。cert表に該当certが残っているかに依存せず、`author_pub` から `node_id = hex(sha256(tag\|\|pub))` を直接再計算してCRLと照合するため、cert表からエントリが失われていても遡及除外は成立する | `src/core/policy.rs::is_author_revoked` / `src/core/sync.rs`(`is_searchable`・`handle_entry_request`・`handle_digest_request`) | 上表「侵害/誤設定ノード」行「既取込分は再検証で除外」を、cert表非依存の直接再計算という粒度まで具体化したもの |
| M-1: CA公開鍵のピン留め+TOFU | ローカル設定(起動時オプション相当)でCA鍵を与えた場合は常にピン留めし、以後レジストリからの供給値を無視する。ローカル未設定の場合のみ、レジストリからの初回の非空CA供給でブートストラップし(TOFU)、以後はレジストリが別のCA鍵を返しても上書きしない | `src/core/policy.rs::set_ca_pub` / `src/core/registry_client.rs` | §0原則(レジストリは発見のみ・信頼は各ノードが自律検証)の具体化。レジストリ侵害時に偽CAへ無条件に横滑りする設計を避ける。ただし未ピンノードの初回ブートストラップ自体は偽装されうる(下記「Phase1既知制約」参照) |
| M-2: shareable単調性保護 | reload時、`shareable` を「再導出値 AND ディスク`state.json`の値」で合成する(disk側`false`は再導出が`true`でも`false`のまま=緩めない。disk側`true`は再導出値をそのまま採用。state.json不在/破損は保守側`false`)。ネット越し受信(ingest)はstateを運ばないため送信者申告に依存せず、受信側の再導出のみで決まる | `src/core/cache.rs::load`(手順9) | S3で`shareable`が伝播ゲート(供出/Digest/announce)に配線されたことで、本ノート §4「S2.5 §13残課題との関係」表High-1行の非単調反転を放置すると、`judge_entry`が共有不可とした緩いエントリがreloadを経て網へ漏れる。回帰テスト `test_monotonic_shareable.rs` で確認 |

**Phase1既知制約(CRL配布は完全性を担保しない)**: CRLはレジストリから無署名・平文HTTPで配布される(`GET /registry/ca`)。レジストリを発見専任に留める原則(→ §0)に沿い、CRL自体にCA署名は付けていない。このため、レジストリがcompromiseされた場合、攻撃者は正規ノードを偽って失効させる「CRL検閲(不正な失効注入)」を成立させうる(`policy.rs` のコード側コメントに記録済み)。Phase1は組織信頼を前提にこのリスクを許容し、Phase2でCA署名付きCRL(または同等の完全性保護)により対処することを残課題として記載する。

**TTL暫定値の記録**: `NodeConfig` の検索除外TTLは `volatile_ttl_secs = 3600`(1時間)/ `slow_ttl_secs = 2,592,000`(30日)をPhase1暫定値としてコード側で設定した(本ノート・S2.5のいずれにも確定値の記載がなかったため)。確定チューニングは §11「要判断点」7(TTL再評価の範囲)と合わせて残課題のまま残す。

---

## 8. Public Phase2への非破壊性(移行ゲート検証)

### 追加で載る(作り直しにならない)ことの確認

| Phase2要素 | 載る場所(確保済み) |
|---|---|
| witness署名 | `mutable_state.witness_sigs`(空Vec)+ `nyllm/witness/v1` タグ予約済み([S2.5_エントリ形式設計.md](./S2.5_エントリ形式設計.md) §11) |
| アンカー参照 | `mutable_state.anchor_proof`(None)+ [設計レビュー_2026-07.md](./設計レビュー_2026-07.md) §4.2二層設計 |
| ステーク | `mutable_state.stake`(None) |
| 評判/独立検証 | `mutable_state.trust`(None) |
| revocation | `nyllm/revocation/v1` タグ予約済み + 新メッセージ型追加 |
| DHT配送 | ワイヤプロトコルに新メッセージ型追加(下記) |

- **署名境界不変**: 上記すべて可変状態側=`core_bytes` に触れない=`entry_id` 不変([S2.5_エントリ形式設計.md](./S2.5_エントリ形式設計.md) 保証済み)。
- **配送抽象のバージョニング**: ワイヤプロトコルを `nyllm-wire/v1` とバージョンタグ付きにし、Phase2で `FindNode`/`WitnessExchange`/`Revoke` 等を追加できる形にする(既存メッセージは変えない)。

### 焼き込むと作り直しになる箇所(警告)→ ポリシー差し替え点(フック)として今設ける

1. **★最重要: レジストリに信頼判断を持たせるな。** 「レジストリに載っている=信頼できる」を検証ロジックに焼き込むと、Phase2でレジストリをDHTに差し替えた瞬間に信頼レイヤが破綻する。レジストリは発見のみ、検証は常に `core_bytes` に対して各ノードが自律実行する(→ §0原則)。
2. **CA検証を必須ハードコードするな。** `author_sig` 検証(S2.5)と `node_cert` 検証(S3)を別レイヤに分離し、`node_cert` 検証を「Companyポリシーのプラグイン」として差せる形にする。Publicでは `node_cert` 検証を witness/評判検証に差し替える(`author_sig` 検証コアは不変)。
3. **時刻をNTP前提でハードコードするな。** `created` の信頼を「NTPだから正しい」と埋めず、「Phase1は時刻検証スキップ(組織信頼)、Phase2はアンカーで検証」をポリシー切替にする。`created` フィールド自体はS2.5で不変。
4. **grow-only前提を検索に焼き込むな。** Phase2 revocationで「失効エントリを検索除外」が要る。検索層に失効フィルタのフックを今から通しておく(Phase1は常にpass)。

この4点を「ポリシー差し替え点(trait/フック)」として今設ければ、Phase2は差し替え+空スロット充填で完結する。

### 実装記録・Phase2申し送り(2026-07-18、脅威モデルレビュー指摘)

上記4「grow-only前提を検索に焼き込むな」のフックとして `RevocationPolicy`(既定 `NoRevocationPolicy`=常にpass)を実装した(`src/core/policy.rs`)。ただし現状の配線には非対称がある: `RevocationPolicy` は検索除外(`is_searchable`)とanti-entropyのプル前フィルタ(`run_anti_entropy_once`)という**受信側**の2経路にのみ配線されており、供出(`handle_entry_request`)・Digest列挙(`handle_digest_request`)という**ソース側**は著者CRL(`is_author_revoked`、ノード単位失効)のみを参照し、エントリ単位の`RevocationPolicy`は参照しない。Phase1は`NoRevocationPolicy`が常にpassのため実害はないが、**Phase2でエントリ単位revocationの中身を実装する際は、ソース側(供出/Digest)にも失効フィルタを追加すべきか(=失効エントリを他ノードへ供出し続けてよいか)を再評価すること**。放置すると「自ノードの検索からは除外するが、他ノードへは供出し続ける」という一貫性の欠如が生じうる。

---

## 9. src/coreモジュール構成

### 移植(poc/から。S2.5形式適用済みの状態で)

```text
src/core/
  embedder.rs      trait + factory
  signer.rs        trait + factory(sign_bytes化・S2.5)
  agent.rs         trait
  triples.rs       案4分解+オントロジー
  volatility.rs    揮発性+共有ゲート
  pipeline.rs      judge_entry
  entry.rs         S2.5: ImmutableCore/MutableState/encode_core/parse_core/entry_id/question_key
  cache.rs         SemanticCache(S2.5 load/verify・全複製検索・冪等マージ)
```

### 新規(S3)

```text
  node.rs             NodeId/鍵ロード/node_cert検証(CA)/CRL照合/mode
  transport.rs        Transport trait(send/recv)+ HttpTransport(axum client)+【テスト用】InMemoryTransport
  wire.rs             nyllm-wire/v1 メッセージ型(Announce/Request/Transfer/Digest)+ シリアライズ
  sync.rs             anti-entropy(Digest交換・欠落プル)・announce処理
  registry_client.rs  レジストリ登録/ピア一覧取得/CA・CRL取得
  daemon.rs           axum サーバ: UI向けAPI + ノード間API の2系統
  policy.rs           §8のポリシー差し替え点(cert検証/時刻検証/失効フィルタ/発見層)。Phase1実装・Phase2差替
  main.rs             --mode 起動・配線
```

別クレート/バイナリ: `registry`(発見専用の軽量axumサービス)。

### デーモンAPI(主要エンドポイント)

UI向け(同一ノード内、Blazorが叩く):

- `POST /v1/ask {question}` → `{hit, answer, entry_id, similarity, shareable, tier}`(検索→ミス時Agent推論→judge→登録→配送announce)
- `GET /v1/entries/{entry_id}` → エントリ詳細(facts/provenance/volatility)
- `GET /v1/status` → mode/ピア数/エントリ数/embedder id

ノード間(core←→core、`nyllm-wire/v1`):

- `POST /wire/announce` ← Announce受信 → 未知なら非同期プル起動
- `GET /wire/entry/{entry_id}` → Transfer(エンベロープ返却)
- `GET /wire/digest` → Digest(同期用要約)

レジストリAPI:

- `POST /registry/join {node_id, url, node_cert}` / `GET /registry/peers` / `GET /registry/ca`(CA公開鍵+CRL)

---

## 10. テスト観点(CLAUDE.md規則4)

マルチノードのテスト戦略はプロセス内シミュレーションとする。`Transport` をtrait化し、テストでは `InMemoryTransport`(ノード間をチャネル/共有マップで繋ぐ)を使い、N個の `SemanticCache` + デーモンロジックを1プロセス内で起動する。HTTP実配線は別途スモークテストとする。

| 観点 | 確認内容 |
|---|---|
| 配送 round-trip | Node A で登録→Announce→Node B がプル→B に同一 `entry_id` が取り込まれる |
| 受信側再導出 | A が `shareable=true` 主張の state を送っても B は Transfer(core only)から自前再導出。改ざんstateを渡す経路が無いことを確認 |
| 冪等マージ | 同一 `entry_id` を2回受信→1件。順序入替でも収束(可換・冪等) |
| 複数版併存 | 同一 `question_key`・異 `created` で2版→両方保持、検索で両方候補 |
| anti-entropy | Announceを落とした(欠落)状態でDigest交換→欠落分がプルされ収束 |
| author_sig/CA検証 | 無効な `node_cert` / CRL失効鍵で署名されたエントリ→受信側drop |
| 改ざん検知(ネット越し) | Transferの`core_b64`を1バイト改変→`entry_id`不一致でdrop / `author_sig`改変→署名失敗でdrop |
| モード分離 | `--mode private` ノードはregistryに現れずTransfer要求の宛先にならない。`private_store`がcompany配送に漏れない |
| TTL除外 | volatile/期限切れslowが検索から除外される(物理削除されないことも確認) |
| ポリシー差替(非破壊性) | `policy.rs` のcert検証をダミーに差し替えても`author_sig`検証コアが不変(Phase2差替の予行) |
| registry実HTTP統合スモーク | `join`→`peers`→`ca`→`refresh_once` の一連を実axum起動で確認(InMemoryTransportでなく実HTTP経路)。CAピン留めノードはレジストリ供給値で上書きされず、未ピンノードはレジストリ初回の非空供給でTOFU固定される(以後の別供給は無視) |
| privateノードの不可視性 | `--mode private` ノードが `GET /registry/peers` に一切現れないことを確認 |
| 無効certのネット経路drop | 期限切れ/別CA署名/mode不許可のcertで署名した相手からのTransfer(`ingest_transfer`)がネット経路で拒否されることを確認 |
| ポリシー差替の実効性 | `TimePolicy`/`RevocationPolicy` を差し替えると実際に判定結果が変わることを確認(フックが名目上のtrait定義に留まらず実効することの検証) |

`Transport` trait化は既存 `Embedder`/`Signer`/`Agent` と同じ「mock/実装を1call pathで差替」思想の踏襲である。`ed25519` feature でも全テストが通ることを確認する(2026-07-18時点: default/`--features ed25519` の両ビルドで `cargo test --workspace` 全緑を実測。`s3_gate_two_or_more_nodes_share`〔`src/tests/test_sync.rs`。3ノードでshareableエントリが全ノードに複製されることをassert〕を含む)。

---

## 11. 要判断点(fable推奨を既定採用。各に採用/代替を明記)

1. 発見トポロジ → **中央レジストリ(発見専用)**(推奨)。複数拠点対応・デーモン+HTTP方針と噛み合う。代替=LAN gossip(単一拠点なら可)。判断要点=拠点構成の実態。
2. 組織CAの調達 → **既存社内PKIを流用**(推奨)。代替=NyLLM独自の軽量CA(レジストリ同梱、PoC検証に手軽)。判断要点=導入先に既存PKIがあるか。
3. 検索の複製方針 → **全複製ローカル検索**(推奨、〜10^5)。代替=部分複製+scatter-gather(超大規模)。Phase1は全複製で確定し、限界は実測で判断する。
4. Transport traitの粒度 → **メッセージ単位send/recv抽象**(推奨、In-memory/HTTP差替)。判断要点=HTTP(axum)かgRPC(tonic)か。推奨はPhase1はaxum+JSON(デバッグ容易・UIと同系統)、性能要求が出た段階でgRPCを検討する。
5. anti-entropyの頻度/方式 → **定期ポーリング+Digestハッシュ比較**(推奨)。代替=Merkleツリー差分(大規模で効率的だが複雑、Phase2)。
6. Private storeの分離度 → **別ディレクトリ+配送層非起動**(推奨)。代替=別マシン完全物理分離(最も安全だが運用が重い)。判断要点=Privateに何を置くか。
7. TTL再評価の範囲 → **Phase1は検索除外のみ(自動再推論なし)**(推奨)。代替=slow満了で自動再Agent推論(コスト増、Phase2の独立検証と一緒に作る方が筋がよい)。

---

## まとめ

核心3点:

1. シビル対策機構は消すが、説明責任機構(author_sig/受信側再導出/モード分離)は残す(組織信頼は「悪意がない」であって「間違えない」ではない)。
2. 中央レジストリは発見のみを担い、信頼は各ノードが `core_bytes` に対し自律検証する(Phase2でレジストリをDHTに差し替えても信頼レイヤは不変=非破壊性の要)。
3. 配送はS2.5のload/verifyをネット越しに呼ぶだけである(単一/マルチノードで検証コードパスを共有する)。

over-engineering回避線=「組織信頼・共通時計・組織PKIで代替できるものは全て代替し、witness/アンカー/ステーク/評判/DHT/eclipse/revocation(エントリ単位)は空スロットのままPhase2送りとする」。§8の4つのポリシー差し替え点(cert検証/時刻検証/失効フィルタ/発見層)を今フックとして設けておけば、Phase2は差し替え+空スロット充填で完結する。

---

## 次アクション

```text
本ノート確定(docs化)→ オーナー承認 → S2.5実装完了+整合確認の後、project-leader経由で
poc-core-dev(src/core移植+S3新規モジュール)/ blazor-ui-dev(モード別UI・デーモンAPI)へ
実装割り当て(+テスト、CLAUDE.md規則4)。実装は本設計承認後の別段階。
```

**完了(2026-07-18)**: 上記の実装割り当ては完了した。`src/core/`(`node.rs`/`transport.rs`/`wire.rs`/`sync.rs`/`registry_client.rs`/`daemon.rs`/`policy.rs`/`main.rs`)+ 別クレート `registry` に実装済み。テストは `cargo test --workspace`(default/`--features ed25519` の両ビルド)で全緑(S3ゲート判定テスト `s3_gate_two_or_more_nodes_share` を含む)。脅威モデルレビュー(2026-07-18)実施済み・判定=承認(修正必須なし)。実装記録・既知制約・Phase2申し送りは §7・§8・§10 を、進捗ステータスの一次情報は [Roadmap.md](./Roadmap.md) §1・§2「S3 P2P化」節を参照。blazor-ui-dev側(モード別UI)は本ノートの対象外の別トラックであり、本完了記録はcoreレイヤ(`src/core/`)に関するものである。
