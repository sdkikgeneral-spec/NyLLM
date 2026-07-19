// 共有由来エントリへの SHARED_THRESHOLD(0.90)配線の境界テスト
// (Architecture §5.1「共有用しきい値 τ≥0.9」/ §7[補完]既知の穴②の解消 /
//  信頼性設計メモ §2 脅威A「誤ヒット」= 精度優先)。
//
// 「共有由来」の定義 = 他ノードから受信したエントリ(定義(a))。
//   - 受信経路(verify_envelope → insert_verified)で取り込まれたエントリは
//     state.origin_received=true となり、検索時の実効しきい値が
//     SHARED_THRESHOLD(0.90)になる。
//   - 自ノードが register で登録したエントリは origin_received=false のまま、
//     従来どおり検索しきい値(既定 LOCAL_THRESHOLD=0.80)。
//
// 検証観点:
//   - ローカルエントリは 0.80(境界含む)でヒットし、0.80未満でミスする
//   - 共有由来エントリは 0.80〜0.90 の帯ではヒットしない(0.90未満は除外)
//   - 共有由来エントリも 0.90 以上(境界含む)ならヒットする
//   - 「sim は高いが実効しきい値未達の共有由来候補」より「sim は低いが
//     実効しきい値を満たすローカル候補」が採用される(精度優先・汚染回避)
//   - origin_received は state.json 経由で reload を跨いで保持される
//   - state.json 不在時は保守側(共有由来扱い=0.90)に倒れる
//
// 類似度の制御: MockEmbedder では 0.80〜0.90 帯の類似度を決定的に作れないため、
// 固定ベクトルを返すテスト用 Embedder(FixedEmbedder)で内積=類似度を直接
// 埋め込む。lookup は正規化済みベクトルの内積のみを使うので、これで境界値を
// 浮動小数の丸めに依存せず正確に踏める(f32 リテラルは f64 経由の内積計算を
// 往復しても同一ビットに戻る)。

use super::common::{envelope_from_core, make_core, new_signer, rfc3339_days_ago, temp_dir, triple};
use crate::cache::{SemanticCache, LOCAL_THRESHOLD, SHARED_THRESHOLD};
use crate::embedder::Embedder;
use crate::entry::{encode_core, entry_id, Tier};
use crate::signer::Signer;
use std::path::PathBuf;
use std::sync::Arc;

// ------------------------------------------------------------------
// テスト用固定ベクトル Embedder
// ------------------------------------------------------------------

// 第1軸 = 「基準質問」との類似度、第2軸 = 「ローカル質問」との類似度。
// 単一エントリ向けクエリは第3軸に残差を置いた単位ベクトル、
// 混合クエリのみ両軸へ同時に類似度を埋め込む(内積のみ使うため非単位で可)。
struct FixedEmbedder;

// x を第1軸類似度とする単位ベクトル(残差は第3軸へ)。
fn unit_x(x: f32) -> Vec<f32>
{
    vec![x, 0.0, (1.0 - x * x).max(0.0).sqrt()]
}

// y を第2軸類似度とする単位ベクトル(残差は第3軸へ)。
fn unit_y(y: f32) -> Vec<f32>
{
    vec![0.0, y, (1.0 - y * y).max(0.0).sqrt()]
}

impl Embedder for FixedEmbedder
{
    fn name(&self) -> &str
    {
        "fixed-test-embedder"
    }

    fn dim(&self) -> usize
    {
        3
    }

    fn encode(&self, text: &str) -> Vec<f32>
    {
        match text
        {
            // エントリ側の質問文
            "基準質問" => vec![1.0, 0.0, 0.0],
            "ローカル質問" => vec![0.0, 1.0, 0.0],
            // 「基準質問」エントリに対するクエリ(第1軸類似度を直接指定)
            "帯クエリ085" => unit_x(0.85),
            "境界クエリ090" => unit_x(0.90),
            "境界クエリ080" => unit_x(0.80),
            "低域クエリ079" => unit_x(0.79),
            // 「ローカル質問」エントリに対する帯クエリ
            "ローカル帯クエリ085" => unit_y(0.85),
            // 混合: 共有由来(基準質問)と sim=0.85、ローカルと sim=0.82
            "混合クエリ" => vec![0.85, 0.82, 0.0],
            // 未知文字列は全エントリと類似度0(候補にならない)
            _ => vec![0.0, 0.0, 0.0],
        }
    }
}

// ------------------------------------------------------------------
// 共通ヘルパー
// ------------------------------------------------------------------

fn fixed_cache(tag: &str) -> (SemanticCache, Arc<dyn Signer>, PathBuf)
{
    let dir = temp_dir(tag);
    let signer = new_signer(&dir, "n1");
    let cache = SemanticCache::new(
        dir.join("store"),
        Arc::new(FixedEmbedder),
        signer.clone(),
        LOCAL_THRESHOLD,
    );
    (cache, signer, dir)
}

// 「基準質問」エントリをネット越し受信経路(ingest_envelope)で取り込む
// = 共有由来(origin_received=true)にする。署名は cache と同じ signer を使う
// (既定ビルドの DummySigner=MAC は他ノード鍵を検証できないため。共有由来の
//  判別は鍵ではなく取り込み経路で記録される=両ビルドで同一挙動)。
fn ingest_base_entry(cache: &mut SemanticCache, signer: &dyn Signer) -> String
{
    let core = make_core(
        "基準質問",
        vec![triple("日本", "首都", "東京")],
        &rfc3339_days_ago(0),
        "permanent",
        Tier::Low,
    );
    let id = entry_id(&encode_core(&core));
    let env = envelope_from_core(&core, signer);
    cache.ingest_envelope(&env, Some(&id)).expect("受信取り込みに成功するはず");
    id
}

// 「基準質問」エントリを自ノード登録経路(register_entry)で登録する
// = ローカル由来(origin_received=false)にする。
fn register_base_entry(cache: &mut SemanticCache) -> String
{
    let e = cache.register_entry("基準質問", "テスト回答", "permanent", false, "テスト用", "test-agent");
    e.entry_id.clone()
}

// ------------------------------------------------------------------
// しきい値定数のピン留め(CLAUDE.md 不変条件)
// ------------------------------------------------------------------

#[test]
fn threshold_constants_pinned()
{
    // 値を変えるなら docs(Architecture §5.1)の根拠ごと更新提案すること。
    assert_eq!(LOCAL_THRESHOLD, 0.80);
    assert_eq!(SHARED_THRESHOLD, 0.90);
    assert!(SHARED_THRESHOLD > LOCAL_THRESHOLD, "共有側が厳しい=精度優先");
}

// ------------------------------------------------------------------
// ローカルエントリ: 従来どおり 0.80 で判定
// ------------------------------------------------------------------

#[test]
fn local_entry_hits_in_band_080_090()
{
    let (mut cache, _signer, _dir) = fixed_cache("local_band");
    register_base_entry(&mut cache);

    // 帯 0.80〜0.90 の類似度でヒットする(ローカルは LOCAL_THRESHOLD)
    let r = cache.lookup("帯クエリ085");
    assert!(r.entry.is_some(), "ローカルエントリは sim=0.85 でヒットするはず");
    assert!((r.similarity - 0.85).abs() < 1e-3, "sim={}", r.similarity);

    // 境界 0.80 ちょうどでもヒット(>= 判定)
    let r = cache.lookup("境界クエリ080");
    assert!(r.entry.is_some(), "ローカルエントリは sim=0.80(境界)でヒットするはず");

    // 0.80 未満はミス(従来挙動の回帰確認)
    let r = cache.lookup("低域クエリ079");
    assert!(r.entry.is_none(), "sim=0.79 はローカルしきい値未満でミス");
    assert!((r.similarity - 0.79).abs() < 1e-3, "ミス時も最良類似度を観測報告する");
}

// ------------------------------------------------------------------
// 共有由来エントリ: 0.80〜0.90 の帯ではヒットしない
// ------------------------------------------------------------------

#[test]
fn shared_entry_misses_in_band_080_090()
{
    let (mut cache, signer, _dir) = fixed_cache("shared_band");
    let id = ingest_base_entry(&mut cache, signer.as_ref());
    assert!(
        cache.get(&id).unwrap().state.origin_received,
        "受信経路で取り込んだエントリは共有由来(origin_received=true)"
    );

    // 帯 0.80〜0.90: ローカルしきい値は超えるが SHARED_THRESHOLD 未満 → 不採用
    let r = cache.lookup("帯クエリ085");
    assert!(r.entry.is_none(), "共有由来エントリは sim=0.85 ではヒットしない(0.90未満は除外)");
    assert!((r.similarity - 0.85).abs() < 1e-3, "ミス時も最良類似度を観測報告する");

    // 境界 0.80 ちょうども当然不採用
    let r = cache.lookup("境界クエリ080");
    assert!(r.entry.is_none(), "共有由来エントリは sim=0.80 ではヒットしない");
}

#[test]
fn shared_entry_hits_at_or_above_shared_threshold()
{
    let (mut cache, signer, _dir) = fixed_cache("shared_hit");
    ingest_base_entry(&mut cache, signer.as_ref());

    // 境界 0.90 ちょうどでヒット(>= 判定)
    let r = cache.lookup("境界クエリ090");
    assert!(r.entry.is_some(), "共有由来エントリは sim=0.90(境界)でヒットするはず");
    assert!((r.similarity - 0.90).abs() < 1e-3, "sim={}", r.similarity);

    // 完全一致(sim=1.0)でもヒット
    let r = cache.lookup("基準質問");
    assert!(r.entry.is_some(), "共有由来エントリも sim=1.0 ならヒットする");
    assert!((r.similarity - 1.0).abs() < 1e-3);
}

// ------------------------------------------------------------------
// 混合: 実効しきい値未達の共有由来候補より、達しているローカル候補を採用
// ------------------------------------------------------------------

#[test]
fn local_qualified_candidate_wins_over_higher_sim_unqualified_shared()
{
    let (mut cache, signer, _dir) = fixed_cache("mixed");
    ingest_base_entry(&mut cache, signer.as_ref()); // 共有由来(基準質問)
    cache.register_entry("ローカル質問", "テスト回答", "permanent", false, "テスト用", "test-agent");

    // 混合クエリ: 共有由来と sim=0.85(0.90未満→不採用)、
    //             ローカルと sim=0.82(0.80以上→採用可)。
    // 全体最良は共有由来(0.85)だが、実効しきい値を満たすローカル(0.82)を返す。
    let r = cache.lookup("混合クエリ");
    let hit = r.entry.expect("実効しきい値を満たすローカル候補が採用されるはず");
    assert_eq!(hit.core.question_norm, "ローカル質問");
    assert!((r.similarity - 0.82).abs() < 1e-3, "採用候補自身の類似度を報告する: {}", r.similarity);
}

// ------------------------------------------------------------------
// 永続化: origin_received は reload を跨いで保持される
// ------------------------------------------------------------------

#[test]
fn origin_survives_reload()
{
    let dir = temp_dir("origin_reload");
    let signer = new_signer(&dir, "n1");
    let store = dir.join("store");
    {
        let mut cache =
            SemanticCache::new(store.clone(), Arc::new(FixedEmbedder), signer.clone(), LOCAL_THRESHOLD);
        ingest_base_entry(&mut cache, signer.as_ref()); // 共有由来
        cache.register_entry("ローカル質問", "テスト回答", "permanent", false, "テスト用", "test-agent");
    } // drop → ディスクからの再ロードで検証する

    let cache = SemanticCache::new(store, Arc::new(FixedEmbedder), signer.clone(), LOCAL_THRESHOLD);
    assert_eq!(cache.size(), 2, "両エントリとも reload で復元される");

    // ローカル由来は reload 後も 0.80 帯でヒット
    let r = cache.lookup("ローカル帯クエリ085");
    assert!(r.entry.is_some(), "ローカル由来は reload 後も sim=0.85 でヒット");

    // 共有由来は reload 後も 0.90 未満で除外
    let r = cache.lookup("帯クエリ085");
    assert!(r.entry.is_none(), "共有由来は reload 後も sim=0.85 ではヒットしない");
}

#[test]
fn missing_state_json_defaults_to_shared_threshold()
{
    // state.json 不在 = 由来(登録時の経路記録)を確認できない → 保守側に倒し
    // 共有由来扱い(SHARED_THRESHOLD)とする(M-2 の shareable cap と同方針)。
    let dir = temp_dir("origin_missing_state");
    let signer = new_signer(&dir, "n1");
    let store = dir.join("store");
    let id;
    {
        let mut cache =
            SemanticCache::new(store.clone(), Arc::new(FixedEmbedder), signer.clone(), LOCAL_THRESHOLD);
        id = register_base_entry(&mut cache); // ローカル由来として登録
    }
    std::fs::remove_file(store.join(format!("{id}.state.json"))).expect("state.json 削除に失敗");

    let cache = SemanticCache::new(store, Arc::new(FixedEmbedder), signer.clone(), LOCAL_THRESHOLD);
    assert!(cache.contains(&id), "core 検証は通るのでエントリ自体は残る");
    assert!(
        cache.get(&id).unwrap().state.origin_received,
        "由来不明は共有由来扱い(保守側)"
    );

    // 由来不明 → 0.80〜0.90 帯ではヒットしない
    let r = cache.lookup("帯クエリ085");
    assert!(r.entry.is_none(), "由来不明エントリは sim=0.85 ではヒットしない");

    // 0.90 以上なら由来不明でもヒットする(完全排除ではなく精度優先)
    let r = cache.lookup("基準質問");
    assert!(r.entry.is_some(), "sim=1.0 なら由来不明でもヒットする");
}
