// 共有キルスイッチ(共有オフ+法的姿勢再定義スペック §2〜§4)の回帰テスト。
//
// NodeService に積んだ sharing_enabled(AtomicBool・既定 true)と、送出系5経路
// (broadcast_announce / handle_entry_request / handle_digest_request /
// run_anti_entropy_once / handle_announce)を1点に集約するゲート
// sharing_active() = delivery.is_some() && is_sharing_enabled() の実効性を検証する。
//
// 既存の H-1(§3(d) ソース側 revocation フィルタ)とは独立に積み上がる機能であり、
// このテストファイルはそれと別の懸念(共有オン/オフの実行中トグル)のみを見る。
// 多層防御の前提: private モードは delivery=None のため sharing_active() は常に
// false(構造的不在)。killswitch は company モードへの runtime 制御を足すもので
// あり、private の構造的不在を置き換えるものではない(本スペック §0 絶対制約)。

use super::common::{build_company_network, temp_dir, SHAREABLE_QUESTION};
use crate::sync::AnnounceOutcome;
use crate::wire::Announce;

// --------------------------------------------------------------
// 1. 既定は on(非破壊の固定)
// --------------------------------------------------------------

#[test]
fn default_sharing_enabled()
{
    let net = crate::transport::InMemoryNetwork::new();
    let dir = temp_dir("killswitch_default");
    let ca = super::common::new_ca(&dir);
    let nodes = build_company_network(&net, &dir, &["a"], &ca);
    let a = &nodes[0];

    assert!(
        a.svc.is_sharing_enabled(),
        "構築直後は既定 true(既存テスト・anti-entropyループを壊さない非破壊固定)"
    );
    // status() にも反映されていること(§2.3)。
    let status = a.svc.status();
    assert!(status.sharing_enabled);
    assert!(status.sharing_active, "company + delivery あり + enabled=true なら active");
}

// --------------------------------------------------------------
// 2. オフで供出・Digest が止まる
// --------------------------------------------------------------

#[test]
fn killswitch_stops_serving()
{
    let net = crate::transport::InMemoryNetwork::new();
    let dir = temp_dir("killswitch_serving");
    let ca = super::common::new_ca(&dir);
    let nodes = build_company_network(&net, &dir, &["a"], &ca);
    let a = &nodes[0];

    let res = a.svc.ask(SHAREABLE_QUESTION).expect("MockAgentは失敗しない");
    assert!(res.shareable, "首都エントリは共有可");
    let eid = res.entry_id.clone();

    // 前提: オフ前は供出・Digest 掲載できる。
    assert!(a.svc.handle_entry_request(&eid).is_some(), "前提: オフ前は供出できる");
    assert!(
        a.svc.handle_digest_request().entries.iter().any(|i| i.entry_id == eid),
        "前提: オフ前は Digest に掲載される"
    );

    a.svc.set_sharing_enabled(false);

    assert!(
        a.svc.handle_entry_request(&eid).is_none(),
        "共有オフ後は handle_entry_request が None"
    );
    assert!(
        a.svc.handle_digest_request().entries.is_empty(),
        "共有オフ後は handle_digest_request が空"
    );
}

// --------------------------------------------------------------
// 3. オフで anti-entropy が即 return(プルしない)
// --------------------------------------------------------------

#[test]
fn killswitch_stops_anti_entropy()
{
    let net = crate::transport::InMemoryNetwork::new();
    let dir = temp_dir("killswitch_antientropy");
    let ca = super::common::new_ca(&dir);
    let nodes = build_company_network(&net, &dir, &["a", "b"], &ca);
    let (a, b) = (&nodes[0], &nodes[1]);

    // announce を疑似ロスさせ、B が anti-entropy でしか取得できない状況を作る。
    net.set_drop_announces(true);
    let _res = a.svc.ask(SHAREABLE_QUESTION).expect("MockAgentは失敗しない");
    assert_eq!(b.svc.entry_count(), 0, "announce 損失で B は未取得(前提)");

    // B の共有をオフにする。
    b.svc.set_sharing_enabled(false);

    let rep = b.svc.run_anti_entropy_once();
    assert_eq!(rep.peers_total, 0, "共有オフでは即 return(ピア走査すらしない)");
    assert_eq!(rep.pulled, 0, "共有オフではプルしない");
    assert_eq!(b.svc.entry_count(), 0, "エントリは取得されないまま");
}

// --------------------------------------------------------------
// 4. オフで announce 受信プルが起動しない
// --------------------------------------------------------------

#[test]
fn killswitch_stops_announce_ingest()
{
    let net = crate::transport::InMemoryNetwork::new();
    let dir = temp_dir("killswitch_announce_ingest");
    let ca = super::common::new_ca(&dir);
    let nodes = build_company_network(&net, &dir, &["a"], &ca);
    let a = &nodes[0];

    a.svc.set_sharing_enabled(false);

    let ann = Announce
    {
        entry_id: "dummy-entry-id".to_string(),
        question_key: "q".to_string(),
        created: "2026-07-20T00:00:00Z".to_string(),
        node_id: "someone".to_string(),
    };
    assert!(
        matches!(a.svc.handle_announce(&ann), AnnounceOutcome::NoDelivery),
        "共有オフでは handle_announce が NoDelivery を返す(プル起動しない)"
    );
}

// --------------------------------------------------------------
// 5. オフでもローカル検索は生存する
// --------------------------------------------------------------

#[test]
fn local_search_survives_killswitch()
{
    let net = crate::transport::InMemoryNetwork::new();
    let dir = temp_dir("killswitch_local_survive");
    let ca = super::common::new_ca(&dir);
    let nodes = build_company_network(&net, &dir, &["a"], &ca);
    let a = &nodes[0];

    let res = a.svc.ask(SHAREABLE_QUESTION).expect("MockAgentは失敗しない");
    let eid = res.entry_id.clone();

    a.svc.set_sharing_enabled(false);

    // lookup 相当(ask の検索経路)が引き続きヒットする。
    let res2 = a.svc.ask(SHAREABLE_QUESTION).expect("MockAgentは失敗しない");
    assert!(res2.hit, "共有オフでもローカル検索(ask)は登録済みエントリにヒットする");
    assert_eq!(res2.entry_id, eid);

    // get 相当(entry_detail)も引き続き取得できる。
    assert!(
        a.svc.entry_detail(&eid).is_some(),
        "共有オフでも entry_detail(get 相当)は取得できる"
    );
}

// --------------------------------------------------------------
// 6. オン→オフ→オンで供出・Digest が可逆に復帰する
// --------------------------------------------------------------

#[test]
fn resume_sharing()
{
    let net = crate::transport::InMemoryNetwork::new();
    let dir = temp_dir("killswitch_resume");
    let ca = super::common::new_ca(&dir);
    let nodes = build_company_network(&net, &dir, &["a"], &ca);
    let a = &nodes[0];

    let res = a.svc.ask(SHAREABLE_QUESTION).expect("MockAgentは失敗しない");
    let eid = res.entry_id.clone();

    a.svc.set_sharing_enabled(false);
    assert!(a.svc.handle_entry_request(&eid).is_none(), "オフ中は供出しない");
    assert!(a.svc.handle_digest_request().entries.is_empty(), "オフ中は Digest 空");

    a.svc.set_sharing_enabled(true);
    assert!(
        a.svc.handle_entry_request(&eid).is_some(),
        "再オンで供出が復帰する(可逆)"
    );
    assert!(
        a.svc.handle_digest_request().entries.iter().any(|i| i.entry_id == eid),
        "再オンで Digest 掲載も復帰する(可逆)"
    );
    assert!(a.svc.is_sharing_enabled());
    assert!(a.svc.status().sharing_active);
}

// --------------------------------------------------------------
// 7. オフ時は登録しても announce 送信数が 0
// --------------------------------------------------------------

#[test]
fn register_skips_announce_when_off()
{
    let net = crate::transport::InMemoryNetwork::new();
    let dir = temp_dir("killswitch_register_announce");
    let ca = super::common::new_ca(&dir);
    // ピアがいる(2ノード)ことで「オフでなければ announce するはず」の前提を明確にする。
    let nodes = build_company_network(&net, &dir, &["a", "b"], &ca);
    let (a, b) = (&nodes[0], &nodes[1]);

    a.svc.set_sharing_enabled(false);

    // AskResult.announced_to(既存の announce 計測フック)で送信数0を確認する。
    let res = a.svc.ask(SHAREABLE_QUESTION).expect("MockAgentは失敗しない");
    assert!(res.shareable, "首都エントリは共有可(オフでも登録自体は行われる)");
    assert_eq!(res.announced_to, 0, "共有オフ時は announce 送信数が0");

    // 登録(ローカルキャッシュへの追加)自体は行われる。
    assert_eq!(a.svc.entry_count(), 1, "共有オフでもローカル登録は行われる");
    // B へは announce が飛んでいない(ピアは取り込んでいない)ことの裏取り。
    assert_eq!(b.svc.entry_count(), 0, "共有オフでは B へ announce/配送されない");
}
