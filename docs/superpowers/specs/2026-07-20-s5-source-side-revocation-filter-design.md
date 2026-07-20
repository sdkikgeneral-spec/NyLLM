# S5 実装スペック — ソース側 revocation フィルタ対称化(§3(d))+ 限定的設計深掘り

> ステータス: **実装前スペック(承認待ち)**(2026-07-20)
> スコープ決定: ユーザー選択「設計深掘り + 安全な1点実装」(2026-07-20)
> 出典: [S5_Public_Phase2_法的機構先行設計.md](../../S5_Public_Phase2_法的機構先行設計.md) §3(d)・§3(h)・§7・§8、[Roadmap.md](../../Roadmap.md) §1 S5行・§2「S3 P2P化」節「残課題」、[S3_Company_Phase1_社内多ノード共有設計.md](../../S3_Company_Phase1_社内多ノード共有設計.md) §8、実コード `src/core/policy.rs` / `src/core/sync.rs`
> 前提: このスペックは S5 全体を実装するものでは**ない**。S5 は定義上 Public Phase2 専属であり、実装着手は移行ゲート5条件+弁護士レビューを経てから。本スペックは (1) S5ノート§3(d)が「要」と結論済みで Phase1 非破壊に実装できる1点、(2) それに付随する限定的な docs 決着、の2つに絞る。

---

## 0. 本スペックが変えないもの(絶対制約)

- **S1〜S7 の段階定義・ゲート・進捗ステータス**([Roadmap.md](../../Roadmap.md) §1・§2)は不変。とくに **S5 ステータス「未着手」・ゲート「失効が全ノードに伝播」は不変**。本スペックのパート1は S5 ゲートを達成しない(Revoke ワイヤ型も tombstone 集合も伝播機構も実装しない)。パート1は **S3 が残した既知課題(ソース側フィルタ非対称)の解消**であって、S5 の前進ではない。
- **Phase1 挙動は完全不変**。追加する参照先は既定 `NoRevocationPolicy`(常に `false`)であり、既定構成では no-op。挙動が変わるのは Phase2 で実 `RevocationPolicy` を注入したとき、およびテストで stub を注入したときのみ。
- **署名・`entry_id`・検証コア**(`cache::verify_envelope` 内のハッシュ照合+`Signer::verify`)には一切触れない。
- **法的助言ではない**。Public 層公開前の専門弁護士レビュー必須という [Architecture.md](../../Architecture.md) §11 / [信頼性設計メモ.md](../../信頼性設計メモ.md) §8 の免責を薄めない。

---

## 1. 背景(なぜこの1点だけを実装するか)

S5ノート§3(d)は、S3 が残した非対称を裏取りの上でこう結論している:

- `RevocationPolicy`(エントリ単位失効)は現状**受信側の2経路のみ**に配線済み — 検索除外(`is_searchable`)・anti-entropy プル前(`run_anti_entropy_once`)。
- **ソース側の2経路**(供出 `handle_entry_request`・Digest 列挙 `handle_digest_request`)は**著者CRL(`is_author_revoked`、ノード単位失効)のみ**を参照し、`RevocationPolicy` を参照しない。
- この非対称のため、失効エントリを既に保持するノードが他ノードへ供出・Digest 列挙を続けうる(§3(d)本文の具体シナリオ参照)。
- ノート§3(d)の結論: **供出・Digest 列挙の2経路は「要(ソース側にも配線すべき)」**。H-1(CRL失効著者を検索・供出・Digest の全経路で遡及除外)と**同じ配線パターンを、ノード単位失効からエントリ単位失効へ拡張するだけ**。新しい仕組みは作らない。

Announce 起点の取り込み経路(`handle_announce`→`ingest_transfer`)への配線は、grow-only 原則との整合検討が必要なため**本スペックでは実装しない**(→パート2 で「配線しない」を推奨として docs 決着)。

---

## 2. パート1: ソース側 revocation フィルタ対称化(コード実装)

### 2.1 変更対象(2箇所のみ)

**ファイル**: `src/core/sync.rs`

#### (a) `handle_entry_request`(現 sync.rs:541 付近)

`is_author_revoked` チェックの直後に、エントリ単位失効チェックを追加する。

```rust
    pub fn handle_entry_request(&self, entry_id: &str) -> Option<Transfer>
    {
        self.delivery.as_ref()?; // 配送層なし(private)は供出しない(§6)
        let cache = self.cache.lock().unwrap();
        let e = cache.get(entry_id)?;
        if !e.state.shareable
        {
            return None;
        }
        if self.policies.cert.is_author_revoked(&e.author_pub)
        {
            return None; // 失効著者のエントリは供出しない(H-1 遡及除外)
        }
        // 【S5 §3(d)】エントリ単位失効(tombstone)も供出しない。
        // H-1(ノード単位)と同じ遡及除外パターンをエントリ単位へ拡張。
        // 既定 NoRevocationPolicy は常に false = Phase1 非破壊。
        if self.policies.revocation.is_revoked(entry_id)
        {
            return None;
        }
        cache.envelope_for(entry_id).map(|envelope| Transfer { envelope })
    }
```

**返し方の決着**: 「応答しない or `failed: revoked`」の2択(ノート§3(d))は、既存の `shareable=false`・`is_author_revoked` がいずれも `None` を返すのと同型に **`None`(= not found と同じ扱い)** を採用する。新エラー型を導入せず最小差分に留める。理由: (1) 既存コードスタイルとの一貫性、(2) 「失効済みである」という情報を要求元へ明示的に返すと、失効の存在を外部に漏らす副次経路になりうる(可用性/検閲の観点で「持っていないふり」の方が保守的)。

#### (b) `handle_digest_request`(現 sync.rs:560 付近)

filter 述語にエントリ単位失効の除外を追加する。

```rust
        let mut items: Vec<DigestItem> = cache
            .entries()
            .iter()
            .filter(|e| e.state.shareable
                && !self.policies.cert.is_author_revoked(&e.author_pub)
                // 【S5 §3(d)】tombstone 済み entry_id を Digest 列挙から除外。
                // 既定 NoRevocationPolicy は常に false = Phase1 非破壊。
                && !self.policies.revocation.is_revoked(&e.entry_id))
            .map(|e| DigestItem
            {
                entry_id: e.entry_id.clone(),
                question_key: e.question_key.clone(),
            })
            .collect();
```

### 2.2 変更しない経路(明示)

| 経路 | 扱い |
|---|---|
| 検索除外 `is_searchable` | 変更なし(既に `is_revoked` 配線済み) |
| anti-entropy プル前 `run_anti_entropy_once` | 変更なし(既に `is_revoked` 配線済み) |
| Announce 起点取り込み `ingest_transfer` | **変更なし**(パート2で「配線しない」を推奨決着。OQ#14) |
| `RevocationPolicy` trait 定義 / `NoRevocationPolicy` | 変更なし(インターフェース不変=非破壊) |

### 2.3 コーディング規約

- コメントは日本語。
- ブレースは Allman(`{` を独立行)。`cargo fmt` は brace 崩しを避けるため無闇にかけない(CLAUDE.md)。

---

## 3. パート1 テスト仕様

**配置**: 既に `src/core/tests/test_revocation.rs` が存在し(`src/core/lib.rs` の `#[cfg(test)] mod tests` に `#[path = "../../tests/test_revocation.rs"]` で配線済み)、CRL/失効系のテストを持つ。ソース側フィルタのテストは**この既存 `test_revocation.rs` に追記する**のが自然(同じ関心領域=失効)。別ファイル `test_revocation_source_side.rs` を新設する場合は `lib.rs` の `mod tests` に `#[path]` 登録を忘れないこと。実装エージェントの判断で既存ファイル追記を優先とする。

**テスト用 stub**: `RevocationPolicy` を実装する `StubRevocationPolicy { revoked: HashSet<String> }` をテストモジュール内に定義し、指定 `entry_id` にのみ `is_revoked=true` を返す。`Policies` へ注入して `NodeService` を構成する(既存の sync/revocation テストのノード構成ヘルパー・`Policies::phase1` の差し替え方に倣う)。既存 `test_revocation.rs` / `test_policy_hooks.rs` に同種の stub があれば再利用する。

**テスト項目**:

1. `source_side_revoke_blocks_entry_request` — shareable かつ著者未失効のエントリを1件登録 → 当該 entry_id を stub の revoked 集合へ入れる → `handle_entry_request(entry_id)` が `None` を返す。
2. `source_side_revoke_excludes_from_digest` — 上記エントリが `handle_digest_request().entries` に含まれないこと(かつ `digest_hash` が除外後の集合と一致)。
3. `no_revocation_policy_serves_and_lists`(no-op 回帰) — 既定 `NoRevocationPolicy` 構成では、同じエントリが `handle_entry_request` で供出され、`handle_digest_request` にも列挙される(従来挙動不変の保証)。
4. `revoke_does_not_physically_delete`(grow-only 保持の確認) — tombstone 済みでも `cache.get(entry_id)` は依然 `Some`(物理削除はしない=grow-only)。「供出・列挙されない」と「保持している」が両立することを固定する。
5. `non_revoked_sibling_still_served` — 同一 question_key の別版(revoked でない)は供出・列挙され続ける(失効はピンポイントで entry_id 単位に効き、巻き添えがない)。

**完了条件**: `cargo test --workspace` が **default / `--features ed25519` の両ビルドで全緑**。既存テスト数(直近 119 passed)+新規5件が緑になること。

---

## 4. パート2: 限定的設計深掘り(docs のみ・コード無し)

S5ノートは意図的に「要判断点(推奨既定採用)節を設けない=open questions を推奨案で強制決着させない」と宣言している(ノート§0)。この方針を尊重し、**弁護士レビュー・外部コスト・S4層2 に依存する OQ は開いたまま**にする。技術・アーキテクチャで閉じられる OQ **のみ**を推奨案で決着させる。

### 4.1 推奨案で閉じる OQ(技術・アーキテクチャで完結)

| OQ | 決着(推奨) | 根拠 |
|---|---|---|
| #5 ソース側フィルタの hot path 性能 | **専用ベンチ不要**。追加した `is_revoked` 照合は、同一経路で既に走る `is_author_revoked` と同格(実装は集合/表の O(1) 照合想定)であり、経路の計算量オーダーを変えない | ソース側2経路には既に H-1 の CRL 照合が入っている。同格の1照合追加であり、独立の性能懸念を生まない |
| #14 ingest 経路(Announce 起点)への配線要否 | **配線しない**を推奨 | grow-only 原則(受信は基本拒絶せず取り込み、伝播可否/可視性は別レイヤで判断)を維持。tombstone は「物理保持を止める」機構ではなく「供出・列挙・検索から外す」機構。物理取り込みを止めると grow-only 設計と衝突し、かつ検索除外(`is_searchable`)が可視性を既に担保しているため二重防御にならない。=パート1で供出・Digest・検索の3経路を塞げば、ingest を塞がなくても「他ノードへ配り続けない」は達成される |

### 4.2 開いたまま残す OQ(今閉じない理由を明記)

- **#1 著作権侵害通知トリガの発行者資格認定** — 法務/組織判断+弁護士レビュー依存。技術で先取り決着すると誤誘導。
- **#2 tombstone 理由コード分類体系** — 上記#1と一体。分類は法的カテゴリに依存。
- **#4 gossip/フラッディングの重複排除**(Public Phase2) — DHT 置換後の物理層に依存。Phase1 物理層では検証不能。
- **#7/#8 regurgitation コーパス調達・言語/範囲/閾値** — 外部サービス調査+コスト試算+手続き的知識レーン(S4依存)待ち。
- **#12 `prompt_hash` プライバシー** — クエリメタデータ追跡可能性([設計レビュー_2026-07.md](../../設計レビュー_2026-07.md) §3.4【5】)と衝突。要方針判断。
- **#13 評判連動スラッシング** — S4 層2(局所EigenTrust)成果物依存。S4 なしに設計不能。
- **#15 tombstone 同期プロトコル** — Revoke digest 新設 or 既存 Digest 拡張の選択は S5 本体(Phase2)の伝播機構設計と一体。パート1のソース側フィルタは「知っている tombstone を配らない」だけで、tombstone 遡及配布(#15)が閉じて初めて「全ノードに伝播」が成立する。パート1では扱わない。
- その他(#3 復権フロー / #6 anchor_proof / #9 R2凍結可否 / #10 sig署名主体 / #11 schema_ver共存 / #16 タグ僭称帰属 / #17 失効DoS / #18 tombstone永続化 / #19 失効の失効 / #20 再登録回避 / #21 facts限定での必須性) — いずれも Phase2 本体設計・法務・S4依存。

### 4.3 docs 反映先(design-docs-editor 担当)

1. **S5ノート**: §3(d) に「供出・Digest 列挙の2経路は Phase1 非破壊形(既定 no-op)で実装済み(2026-07-20)」の実装記録を追記。§7 非破壊性の記述と整合。§8 総覧の #5・#14 を「技術決着済み(推奨案採用)」へ更新、#5 は「実装により解消」、#14 は「配線しない=決着」。他 OQ は現状維持。
2. **Roadmap.md §2「S3 P2P化」節「残課題」**: 「エントリ単位失効は受信側フィルタのみに配線…ソース側フィルタの要否を再評価」の行に、「**供出・Digest 列挙の2経路は 2026-07-20 に配線済み(Phase1 非破壊・既定 no-op)。ingest 経路は grow-only 原則によりあえて非配線=S5ノート§3(d)・OQ#14 で決着**」の更新を追記。**S3 ステータス「完了(Company Phase1縮約範囲)」は不変**。
3. **Roadmap.md §2「S5 法的機構」節 / §1 S5行**: 「ソース側 revocation フィルタ対称化を横断実装(S3実装の延長・S5ゲートは未達のまま)」の注記を追加。**S5 ステータス「未着手」・ゲート「失効が全ノードに伝播」は不変**という一文を必ず併記。
4. **横断注記の型**: Roadmap の既存注記(2026-07-19 Ollama、2026-07-20 SHARED_THRESHOLD 配線)と同じく「これは S1〜S7 の段階定義・ゲート・進捗ステータスを変更しない」を明記した横断注記として書く。

---

## 5. 実装体制・順序(CLAUDE.md 準拠)

承認後、**project-leader 経由**で以下へ割り当てる。実装エージェントは **model=fable**(メモリ確定フィードバック)。

1. `poc-core-dev`(fable): パート1(sync.rs 2箇所)+ テスト(§3)を実装。両ビルドで `cargo test --workspace` 緑を確認。
2. `threat-model-reviewer`: パート1 diff を脅威モデル観点でレビュー(cache/署名/失効/共有ゲート該当)。とくに「返し方=`None`」の情報漏洩観点、no-op 回帰、grow-only 保持の維持を確認。
3. `design-docs-editor`: パート2(§4.3 の docs 反映)。S1〜S7 の段階定義・ゲート・ステータス不変を厳守。

**注意**: `cargo fmt` を brace 崩し目的でかけない(Allman は手動維持)。

---

## 6. 完了条件(このスペックの Done)

- [ ] パート1: sync.rs 2箇所の配線 + `test_revocation_source_side.rs` 5件、両ビルドで `cargo test --workspace` 全緑。
- [ ] 脅威モデルレビュー: ブロッカーなし(Critical/High の新規指摘なし)。
- [ ] パート2: S5ノート・Roadmap への docs 反映(S5「未着手」・S3「完了」・全ゲート不変)。
- [ ] 不変条件(§0)がすべて維持されていることの確認。

---

## 7. 明示的な非目標(このスペックでやらないこと)

- Revoke ワイヤメッセージ型の追加(S5 本体・Phase2)。
- tombstone 集合そのものの実装・永続化・同期(#15・#18、Phase2)。
- 権限モデル・理由コード・第三者失効(#1・#2・#19、法務+Phase2)。
- R2 regurgitation フィルタ・R4 出所記録拡張(§4・§5、Phase2)。
- S5 ゲート「失効が全ノードに伝播」の達成(Phase2)。
- ingest 経路への `RevocationPolicy` 配線(§4.1 OQ#14 で「配線しない」決着)。
