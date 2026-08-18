//! 构图函数：**建 atom 的唯一入口**。
//!
//! 命令层写槽位、applier 的 `resolve` 重建被逐出的槽位、derived 的 read fn 现查
//! 依赖——三条路径调的是同一个 [`source_atom`]。019 的「重建走正常创建路径，
//! 不是特判分支」因此是字面意义上的同一行代码，不是纪律：applier 里根本没有
//! 「不存在就怎样」的分支，它只调 `resolve`，而 `resolve` 就是这个函数。
//!
//! ## 红线 4 的孪生条款（019 实测钉住）
//!
//! derived 的 read fn **不得捕获 `AtomId`**，一律按逻辑键现查 family。捕获 id 的
//! derived 在依赖被逐出重建后当场 panic（`AtomId` 单调不复用，死 id 不会重指到
//! 别人身上——幸而不是静默错值）。本文件里 [`tools_converged_read`] 捕获的是
//! `AgentId` + 两个句柄（`Store` / family 的 `Rc`），**没有一个 `AtomId` 被捕获**。
//!
//! ## 红线 1
//!
//! read fn 里没有时钟、没有随机数、没有 IO——恢复 = 从快照重放日志，重放要能得出
//! 同样的结果。`scripts/check-invariants.sh` 的 `check_derived_purity` 粗筛路径
//! 覆盖本目录。
//!
//! ## 一个借用陷阱（019 推给本 issue 的账）
//!
//! [`source_atom`] 会 `family.borrow_mut()`。**调用它的时候手上不能已经握着
//! family 的借用**，也不能在 applier 的 `resolve` 闭包里顺手 `store.get(某个
//! derived)`——那个 derived 又要现查 family，就是 `already mutably borrowed`。
//! 本文件的两个函数都借完即还，读值一律走 `args.get`（derived 内）或调用方
//! 自己的 `store.get`（derived 外）。

use std::cell::RefCell;
use std::rc::Rc;

use agent_store::{AtomFamily, AtomId, ReadArgs, Store};

use crate::engine::state::{SlotState, TurnStatus};
use crate::ids::AgentId;
use crate::value::atom_value::AgentValue;
use crate::value::awaiting::AwaitUntil;

use super::atom_key::{AtomKey, DerivedKey};
use super::slot::Slot;

/// 整棵 agent 树共用的这一个 store（STATE-MODEL §「子 agent」）。
pub type AgentStore = Store<AgentValue>;

/// source（primitive）槽位的 family。`Rc<RefCell<_>>` 是因为 derived 的 read fn
/// 要在**重算时**现查它，而重算发生在 store 内部——除了共享内部可变性没有别的
/// 办法把同一张表同时交给命令层和闭包。
pub type SourceFamily = Rc<RefCell<AtomFamily<AtomKey>>>;

/// derived atom 的 family。跟 source 分开是为了让「快照只存 primitive」成为
/// 类型上的事实：`Session::primitives()` 遍历的是前者，它装不下一个 derived。
pub type DerivedFamily = Rc<RefCell<AtomFamily<DerivedKey>>>;

/// 按逻辑键拿一个 source atom，没有就按 [`AtomKey::default_value`] 建一个。
///
/// **get-or-create 是刻意的**：019 的 applier 把它当 `resolve` 用，于是「这个 atom
/// 早就被逐出了」在 undo/redo 路径上根本不是一种情况。
pub fn source_atom(store: &AgentStore, family: &SourceFamily, key: &AtomKey) -> AtomId {
    family
        .borrow_mut()
        .get_or_create(key.clone(), || store.create_atom(key.default_value()))
}

/// 按逻辑键拿一个 derived atom，没有就建。
///
/// 建出来是 **lazy** 的（`create_derived_ctx`）：第一次读才算，也才装上反向边。
/// `Session::new` 建完图之后会读一次，把边接上——不读的话它对下游是不可见的，
/// 而 019 实测的逐出策略正是靠「还有没有人读它」驱动的。
pub fn derived_atom(
    store: &AgentStore,
    sources: &SourceFamily,
    derived: &DerivedFamily,
    key: &DerivedKey,
) -> AtomId {
    // 212 起这里是 `match` 而不是一个不可反驳的 `let`——**加第二种 derived 时
    // 编译器会在这儿逼下一个人回答「你这个 derived 读了什么」**，而那正是红线 10
    // 唯一还有意义的那半条判据的落点（见模块文档）。
    match key {
        DerivedKey::ToolsConverged(agent) => {
            let (agent, store_h, family_h) = (agent.clone(), store.clone(), sources.clone());
            derived.borrow_mut().get_or_create(key.clone(), || {
                store.create_derived_ctx(move |args| {
                    tools_converged_read(args, &store_h, &family_h, &agent)
                })
            })
        }
        DerivedKey::AwaitReached { target, until } => {
            let (target, until) = (target.clone(), *until);
            let (store_h, family_h) = (store.clone(), sources.clone());
            derived.borrow_mut().get_or_create(key.clone(), || {
                store.create_derived_ctx(move |args| {
                    await_reached_read(args, &store_h, &family_h, &target, until)
                })
            })
        }
    }
}

/// `srv:agent/await` 那个 derived 的 read fn（212）：**读目标的 `Status`，一格**。
///
/// # 这是全系统第一条跨 agent 的边
///
/// 三态，不是两态：
///
/// | 值 | 意思 | 等待方该做什么 |
/// |---|---|---|
/// | `Bool(true)` | 到了 | 收敛，成功 |
/// | `Bool(false)` | 目标已经落终态，但**不是**你等的那一种 | 收敛，`is_error`——**继续等就是永远等** |
/// | `Pending` | 还没到 | 接着等（沿依赖图往下游传，同 `tools_converged`） |
///
/// 中间那一档是刻意的：`until = Done` 而目标 `Failed` 收场时，等待方必须**立刻**
/// 知道等不到了。两态的话它只能一直 `Pending`，而泵会安静地返回，留下一个永远
/// 挂着的槽——正是 212 要防的那类死等。
///
/// # 为什么它证明得了「边只许指向 primitive」
///
/// 这里 `args.get` 的入参只可能是 `source_atom(...)` 的产物，而 `source_atom` 的
/// 键是 `AtomKey`，永远落在 source family 上（`build.rs` 里 source 与 derived 是
/// 两张按不同键类型索引的表——「快照只存 primitive」是**类型上的事实**）。
/// primitive 没有出边，所以这条跨 agent 的边是一条**长度 1 的悬边**，绕不回来。
///
/// 决策 35 之前红线 10 的论证是「两个方向可读的槽位集合不相交 ⇒ 无环」，
/// 而那个论证的前提在这个仓里从来没成立过：跨 agent 读走的是命令层的非追踪读
/// （`cross_read`），一条边都不建。方向约束防的是一类当时还不存在的边。
fn await_reached_read(
    args: &ReadArgs<'_, AgentValue>,
    store: &AgentStore,
    family: &SourceFamily,
    target: &AgentId,
    until: AwaitUntil,
) -> AgentValue {
    // 现查，不捕获 id（红线 4 的孪生条款）。
    let status_id = source_atom(store, family, &AtomKey::Agent(target.clone(), Slot::Status));
    let value = args.get(status_id);
    let Some(status) = value.as_status() else {
        // 同 `tools_converged_read`：唯一合法的走到这里的路径是 DV-3 的故障占位，
        // read fn 在这里**不许 panic**（`agent-store` 的 read 契约）。
        debug_assert!(args.is_faulted(), "Status 槽位只可能持 Status：{value:?}");
        return AgentValue::Null;
    };
    let hit = match until {
        AwaitUntil::Settled => status.is_terminal(),
        AwaitUntil::Done => matches!(status, TurnStatus::Done { .. }),
        AwaitUntil::Failed => matches!(status, TurnStatus::Failed(_)),
    };
    if hit {
        AgentValue::Bool(true)
    } else if status.is_terminal() {
        // 它收场了，但不是你等的那一种——**等不到了**，别再挂着。
        AgentValue::Bool(false)
    } else {
        AgentValue::Pending
    }
}

/// 003 预言的那个 derived 的 read fn：**扫槽位**，没有一个还是 `Pending` 就算收敛。
///
/// 形状是刻意的：扫，不是维护一个计数器。计数器是 undo 之后最容易对不上的东西——
/// 回滚了槽位却没回滚计数，收敛条件就永远差一格或早满一格，而且不报错。搬进原子图
/// 之后连「忘了维护」都不可能了：这里没有可维护的状态，只有一次重算。
///
/// 未收敛答 [`AgentValue::Pending`] 而不是 `Bool(false)`：`Pending` 是「还在等」的
/// 专用值，沿依赖图往下游传播（STATE-MODEL §「Pending 的来历」）。M3 的
/// 「等所有子 agent 完成」会在同一个位置汇聚，那时下游读到的仍然是这一个值。
fn tools_converged_read(
    args: &ReadArgs<'_, AgentValue>,
    store: &AgentStore,
    family: &SourceFamily,
    agent: &AgentId,
) -> AgentValue {
    // 现查，不捕获 id（孪生条款）。借用在这一行结束。
    let slots_id = source_atom(
        store,
        family,
        &AtomKey::Agent(agent.clone(), Slot::ToolSlots),
    );
    let value = args.get(slots_id);
    let Some(slots) = value.as_slots() else {
        // 唯一合法的走到这里的路径是 DV-3 的故障占位（超递归预算时 tracked getter
        // 返回 `Null`，本次运行的结果不会被提交，store 会把依赖算好再重跑）——
        // read fn 在这里**不许 panic**（`agent-store` 的 read 契约）。
        debug_assert!(args.is_faulted(), "ToolSlots 槽位只可能持 Slots：{value:?}");
        return AgentValue::Null;
    };
    if slots
        .iter()
        .any(|slot| matches!(slot.state, SlotState::Pending))
    {
        AgentValue::Pending
    } else {
        // 零个槽位也算收敛（没有东西要等）。真正「该不该继续」由转移表决定：
        // 它只在 `ToolsPending` 下问这个问题，那时至少有一个槽。
        AgentValue::Bool(true)
    }
}

/// 一个 agent 的整张图：九个 source 槽位 + 一个 derived，一次建齐。
///
/// **不 lazy 建 source 槽位**是为了让「完整状态 = 所有 primitive」在
/// `Session::primitives()` 那一侧立刻成立：懒建的话，一个从没被写过的槽位不在
/// family 里，快照就少一项，恢复时那一项落默认值——碰巧默认值就是它当时的值，
/// 于是永远不报错，直到某天默认值改了。
pub fn build_agent(
    store: &AgentStore,
    sources: &SourceFamily,
    derived: &DerivedFamily,
    agent: &AgentId,
) {
    for slot in Slot::ALL {
        let _ = source_atom(store, sources, &AtomKey::Agent(agent.clone(), slot));
    }
    let converged = derived_atom(
        store,
        sources,
        derived,
        &DerivedKey::ToolsConverged(agent.clone()),
    );
    // 读一次把反向边装上（`create_derived_ctx` 是 lazy 的）。
    let _ = store.get(converged);
}
