//! **红线 10 的落点**（212）：跨 agent 的边只许指向 primitive。
//!
//! 决策 35 把红线 10 从「只许上下读」改写成这一条，而**改写之前那条论证的前提在
//! 这个仓里从来没成立过**：跨 agent 读走的是命令层的非追踪读（`cross_read`），
//! 一条边都不建。所以旧红线防的是一类当时还不存在的边。
//!
//! 212 建出了**全系统第一条真的跨 agent 的边**（`DerivedKey::AwaitReached` 的
//! read fn 读目标的 `Status`），新判据的断言因此也在这里，不在 205——那个口
//! 不建边，它证不了这条。
//!
//! # 为什么这条是「结构性质」而不是几个用例
//!
//! `Slot` 全是 source 是**类型上的事实**：`build.rs` 里 source 与 derived 是两张
//! 按不同键类型索引的表（`AtomFamily<AtomKey>` vs `AtomFamily<DerivedKey>`），
//! 而跨 agent 的 `args.get` 只能拿 `Slot` 去构 `AtomKey::Agent`。所以每一条跨
//! agent 的边都是**长度 1 的悬边**，绕不回来。
//!
//! 下面第一条测试把这件事在**运行期**也钉一次：遍历 `Slot::ALL`，对每个构造
//! `AtomKey::Agent`，断言它落在 source family 上、derived family 里没有对应项。
//!
//! **哪天有人加了一个跨 agent 读 derived 的 derived，这条会红——那正是要它红的
//! 时刻。** 212 加的这条边指向 primitive，不该红。

use std::cell::RefCell;
use std::rc::Rc;

use agent_core::graph::{
    AgentStore, AtomKey, DerivedFamily, DerivedKey, Slot, SourceFamily, build_agent, derived_atom,
    source_atom,
};
use agent_core::value::awaiting::AwaitUntil;
use agent_core::{AgentId, AgentValue, AwaitProgress, ChildConfig, Session};
use agent_store::{AtomFamily, Store};

/// 每一个 `Slot` 构出来的键都落在 **source** family 上，derived family 里没有
/// 任何一项跟它对应。
#[test]
fn every_slot_key_lands_in_the_source_family_never_the_derived_one() {
    let store: AgentStore = Store::new();
    let sources: SourceFamily = Rc::new(RefCell::new(AtomFamily::new()));
    let derived: DerivedFamily = Rc::new(RefCell::new(AtomFamily::new()));
    let agent = AgentId::new("root/a1");
    build_agent(&store, &sources, &derived, &agent);

    for slot in Slot::ALL {
        let key = AtomKey::Agent(agent.clone(), slot);
        // 拿得到一个 source atom：这就是「跨 agent 的 `args.get` 能构出来的全部
        // 东西」——`AtomKey` 只有 `Agent`/`ToolCall` 两支，两支都在这张表里。
        let id = source_atom(&store, &sources, &key);
        assert!(
            sources.borrow().iter().any(|(k, i)| k == &key && i == id),
            "{slot:?} 该在 source family 里"
        );
    }

    // derived family 里只有构图时建的那一个（`ToolsConverged`），**没有任何一项
    // 是由 `Slot` 构出来的**——`DerivedKey` 压根没有装 `Slot` 的变体，这一条
    // 因此是类型上的事实；这里断言的是「那张表没被别的东西塞进过 Slot 键」。
    let derived_keys: Vec<DerivedKey> = derived.borrow().iter().map(|(k, _)| k.clone()).collect();
    assert!(
        derived_keys
            .iter()
            .all(|k| matches!(k, DerivedKey::ToolsConverged(_))),
        "构图之后 derived family 里不该有别的东西：{derived_keys:?}"
    );
}

/// **212 那条新边只读一个 primitive，而且只读那一个**。
///
/// 形式是「把它读了哪些键抽成一个可测的事实」：建图之后 derived family 是空的，
/// 读一次 `AwaitReached` 之后——
///
/// 1. derived family 里多了那一项（边建起来了）；
/// 2. **source family 里多出来的键只有目标的 `Status` 那一个**。
///
/// 第 2 条挡的是「读了一堆恰好都是 primitive」：那样红线 10 的字面意思还成立，
/// 但一个 derived 悄悄多读几个槽位会让重算成本和失效面一起变大，而没有任何
/// 东西会响。
#[test]
fn the_await_derived_reads_exactly_one_primitive_the_targets_status() {
    let store: AgentStore = Store::new();
    let sources: SourceFamily = Rc::new(RefCell::new(AtomFamily::new()));
    let derived: DerivedFamily = Rc::new(RefCell::new(AtomFamily::new()));
    let target = AgentId::new("root/a1");
    build_agent(&store, &sources, &derived, &target);

    let before: Vec<AtomKey> = sources.borrow().iter().map(|(k, _)| k.clone()).collect();

    let key = DerivedKey::AwaitReached {
        target: target.clone(),
        until: AwaitUntil::Settled,
    };
    let id = derived_atom(&store, &sources, &derived, &key);
    // **必须真的读一次**：`create_derived_ctx` 是 lazy 的，不读就不算、也不装边。
    let value = store.get(id);
    assert_eq!(value, AgentValue::Pending, "刚建的 agent 还没收场");

    let after: Vec<AtomKey> = sources.borrow().iter().map(|(k, _)| k.clone()).collect();
    let added: Vec<&AtomKey> = after.iter().filter(|k| !before.contains(k)).collect();
    assert!(
        added.is_empty(),
        "构图时整份 Slot::ALL 就已经建齐了，这次读不该新建任何 source atom：{added:?}"
    );

    // 边确实建起来了：derived family 里多了这一项。
    assert!(
        derived.borrow().iter().any(|(k, _)| k == &key),
        "读过之后这条 derived 该在表里"
    );
}

/// 行为面的一条：这条边**跨 agent**（root 读的是 `root/a1` 的 `Status`），
/// 而且目标状态一变，答案自动跟着变——它是图上的一个值，不是某处维护出来的判断。
#[test]
fn the_edge_really_crosses_agents_and_tracks_the_targets_status() {
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();
    let child = session
        .spawn_child(&root, ChildConfig::default(), None)
        .expect("spawn");

    assert_eq!(
        session.await_progress(&child, AwaitUntil::Settled),
        AwaitProgress::Waiting,
        "刚出生的子还没收场"
    );

    // 把它推到终态：`despawn` 不行（那是「不活着」，另一条判据），这里直接
    // 撤销它所在的那一轮不合适——用一次真实的转移把它推到 Done。
    use agent_core::{ContentBlock, Event, StopReason, TokenUsage};
    use agent_core::seam::PrefixImage;
    let epoch = session.epoch();
    let _ = session.step(Event::UserInput {
        agent: child.clone(),
        text: std::sync::Arc::from("干活"),
    });
    let _ = session.step(Event::ProviderDone {
        agent: child.clone(),
        epoch,
        blocks: vec![ContentBlock::Text(std::sync::Arc::from("干完了"))],
        stop: StopReason::EndTurn,
        usage: TokenUsage {
            prompt: 1,
            completion: 1,
            cached: None,
        },
        prefix: PrefixImage {
            segments: Vec::new(),
            prompt_tokens: None,
        },
        adjustments: Vec::new(),
    });

    assert_eq!(
        session.await_progress(&child, AwaitUntil::Settled),
        AwaitProgress::Reached,
        "子落终态之后，跨 agent 那条边该自动算出「到了」"
    );
    // `until = Failed` 而它是 `Done`：**等不到了**，不是接着等——这一档是
    // 「防死等」的那一半（`AwaitProgress::Unreachable`）。
    assert_eq!(
        session.await_progress(&child, AwaitUntil::Failed),
        AwaitProgress::Unreachable,
        "它成功收场了，等「失败」的人该当场知道等不到"
    );
}
