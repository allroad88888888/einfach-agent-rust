//! 212 验收「等的对象死了」的核心层落点，外加一条**独立测试期间发现的边界**
//! （详见文件末尾）。
//!
//! 规格原文（issue 212 §做什么 4、§验收）：「目标被 despawn / 撤销 / 随 turn
//! 收尾拆掉 → 等待方的槽必须收敛成 is_error，不能永远 Pending」，并点名
//! 这条要照抄 `collect_tool.rs:140` 的 `is_live` 闸——即：**光看
//! `await_progress` 是不够的**，因为一个被 despawn 的 agent，它的 `Status`
//! 会被 teardown 写回默认值 `TurnStatus::Idle`（`slot_default.rs`），而
//! `Idle` 不是终态——`await_progress` 因此会一直答 `Waiting`，跟"还没到"完全
//! 分不清。这份测试钉住的正是这个事实：runtime 的收敛逻辑**必须**另外去问
//! `Session::is_live`，不能只靠这个 derived。

use agent_core::value::awaiting::AwaitUntil;
use agent_core::{AgentId, AwaitProgress, ChildConfig, Session};

/// 目标死后，`is_live` 正确翻转成 false——runtime 的 `is_live` 闸靠的正是
/// 这个信号。
#[test]
fn is_live_flips_to_false_after_the_target_is_despawned() {
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();
    let a = session
        .spawn_child(&root, ChildConfig::default(), None)
        .unwrap();
    let b = session
        .spawn_child(&root, ChildConfig::default(), None)
        .unwrap();

    session
        .await_agent(&a, &b, AwaitUntil::Settled)
        .expect("建边只读 AwaitingOn/查环，不碰 derived，不该妨碍随后的 despawn");
    assert!(session.is_live(&b));

    let _ = session.despawn_child(&b).expect("这次 despawn 不该被拒");
    assert!(!session.is_live(&b), "despawn 之后 is_live 该翻转成 false");
}

/// **光靠 `await_progress` 分不清"死了"和"还没到"**——这正是 is_live 闸
/// 存在的理由。目标被 despawn 之后，它的 `Status` 槽回到默认值 `Idle`，
/// `await_progress` 因此答 `Waiting`，跟一个从没开始跑的活 agent 长得一样。
/// 一个只信这个 derived 的收敛逻辑会让等待方永远 `Pending`——正是规格原文
/// 点名要避免的那种静默挂死。
#[test]
fn await_progress_alone_cannot_tell_a_dead_target_from_one_that_has_not_started() {
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();
    let a = session
        .spawn_child(&root, ChildConfig::default(), None)
        .unwrap();
    let b = session
        .spawn_child(&root, ChildConfig::default(), None)
        .unwrap();

    session.await_agent(&a, &b, AwaitUntil::Settled).unwrap();
    let _ = session.despawn_child(&b).expect("despawn 不该被拒");

    assert_eq!(
        session.await_progress(&b, AwaitUntil::Settled),
        AwaitProgress::Waiting,
        "这条断言就是问题本身：derived 单独看不出「死了」，答的是 Waiting"
    );
    assert!(
        !session.is_live(&b),
        "但 is_live 知道——这就是 runtime 必须额外查它的原因"
    );
}

/// **一个正被 `await` 盯着的目标，`despawn` 必须拆得掉**（212 独测发现、
/// 主线修掉的那个真 bug）。
///
/// 这条测试原先钉的是相反的行为：读过一次 `await_progress` 之后 despawn 被
/// `StillRead` 拒绝。那**不是设计意图，是漏了一类「自己人」**——
/// `AwaitReached` 的 derived 键带着目标，而 `despawn` 的读者预检当时只把
/// `ToolsConverged` 当自己人。
///
/// 后果是死等，而且是 212 明明想防住的那一种，只是从另一扇门进来：运行时每转
/// 一圈都要读一次目标的进度（那正是「等到了没有」的判据），读一次就建起一条
/// 边，于是轮末 `orphan::reap` 恒回 `Kept`——那个后台子以活着的状态跨过这一轮、
/// 又再也没有人驱动它，**等待方的槽从此永远 `Pending`**。
///
/// 修法在 `despawn.rs::own_derived_keys`：目标在子树里的 `AwaitReached`
/// （三档 `until` 各一条）跟 `ToolsConverged` 一样算自己人，预检放行、逐出时
/// 一起销。
///
/// 等待方不会因此读到一份幽灵状态：`await_slot::decide` **先判 `is_live`**，
/// 目标一旦不活就当场把槽收敛成 `is_error`，根本不去问那个 derived——这也正是
/// 上面那条测试说的「光靠 `await_progress` 分不清死了和还没到」。
#[test]
fn a_target_that_is_actively_awaited_can_still_be_despawned() {
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();
    let a = session
        .spawn_child(&root, ChildConfig::default(), None)
        .unwrap();
    let b = session
        .spawn_child(&root, ChildConfig::default(), None)
        .unwrap();

    session.await_agent(&a, &b, AwaitUntil::Settled).unwrap();
    // 模拟 runtime 的 harvest 已经轮询过（`await_progress` 是那条轮询唯一会调的
    // 公开读口）——**这一下会建起那条跨 agent 的边**，正是当年挡住 despawn 的东西。
    assert_eq!(
        session.await_progress(&b, AwaitUntil::Settled),
        AwaitProgress::Waiting
    );

    let report = session
        .despawn_child(&b)
        .expect("正被 await 盯着的目标必须拆得掉——拆不掉就是永远挂着的槽");
    assert_eq!(report.agents, vec![b.clone()]);
    assert!(!session.is_live(&b), "拆完 is_live 该翻转，runtime 的闸靠它");

    // 三档 `until` 都要能拆——`AwaitReached` 的键带 `until`，只销一档的话
    // 另外两档会在 `destroy_atom` 上留反向边（那是 panic，不是拒绝）。
    let c = session
        .spawn_child(&root, ChildConfig::default(), None)
        .unwrap();
    for until in AwaitUntil::ALL {
        let _ = session.await_progress(&c, until);
    }
    let _ = session
        .despawn_child(&c)
        .expect("三档都被读过之后照样拆得掉");
    assert!(!session.is_live(&c));
}
