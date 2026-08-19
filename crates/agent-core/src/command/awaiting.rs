//! 等待图：**谁在等谁**，以及**建立那一刻就查环**（212，决策 35 §一）。
//!
//! 这个文件只做这一件事的一条命令 + 两个读口。「等到了怎么把槽位收敛」是运行时
//! 的事（`agent_runtime::await_tool`），core 不驱动任何东西（红线 7）。
//!
//! # 为什么必须在**建立**那一刻挡，不能卡住之后再救
//!
//! 两个互等的 agent 都在等一个 derived、都没有 provider 调用在飞——泵的静止条件
//! 是「两张在飞表都空」，于是它会**安静地返回**，留下两个永远 `Pending` 的槽。
//! **没有 panic、没有超时、没有告警**：卡住之后没有任何人有能力发现它。
//!
//! 所以判据是：`await(target)` 之前，从 `target` 出发顺着等待边走——**走得回调用者
//! 就是环**，当场拒。三角环（A→B→C→A）跟直接互等是同一条路径覆盖的，只查
//! 「目标是不是直接在等我」会漏掉它。
//!
//! # 为什么它是 journaled 状态，不是内存里的一张表
//!
//! **恢复之后还得查得了环。** 放内存里，一次崩溃恢复就把查环能力丢了，而丢了不
//! 报错——恢复出来的会话上一条本该被拒的反向 `await` 会被放行，然后两个 agent
//! 互相等到天荒地老。
//!
//! 顺带白拿：`/undo` 掉建立 `await` 的那一轮，等待边跟着消失，反向 `await` 从此
//! 放行——这一条一行代码都不用写。
//!
//! # 这不是依赖环
//!
//! 依赖环由红线 10 的另一半管（跨 agent 的边只许指向 primitive，落点在
//! `graph::build` 的新 derived）。等待环在依赖图上**一条边都不多**：它是「谁在等
//! 谁」这件事本身成的环。两者同名不同物，别混。

use crate::graph::{AtomKey, DerivedKey, Slot, derived_atom};
use crate::value::atom_value::AgentValue;
use crate::ids::AgentId;
use crate::value::awaiting::{self, AwaitUntil, Awaiting};

use super::session::Session;

/// 建立一条等待边被拒的理由。
///
/// 全部是**显式变体**：拒绝的原因决定模型下一步怎么办（换个目标、还是压根不该等），
/// 糊成一个 `false` 之后只能靠猜。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AwaitDenied {
    /// 等自己。永远等不到——这个 agent 正在跑，它不可能在等待期间落终态。
    Yourself { agent: AgentId },
    /// 这个 id 不在本会话这棵树上。
    NotInSession { target: AgentId },
    /// 在树上，但已经不活着。等一个死掉的 agent 是永远等。
    NotLive { target: AgentId },
    /// **会成环**。`chain` 是从 `target` 走回调用者的那条链（含两端），
    /// 原样交给调用方去组给模型看的话——**拒绝要说清是谁在等谁**，
    /// 只说「不行」模型只会换个写法再撞一次。
    WouldCycle { chain: Vec<AgentId> },
}

/// 一次「等到了没有」的回答。三态，不是布尔——见 `graph::build::await_reached_read`。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AwaitProgress {
    /// 到了。
    Reached,
    /// 目标已经收场，但**不是**你等的那一种（`until = Done` 而它 `Failed` 之类）。
    /// **继续等就是永远等**，调用方必须当场收敛成一个错误。
    Unreachable,
    /// 还没到，接着等。
    Waiting,
}

impl Session {
    /// `target` 到达 `until` 了吗——**读那个 derived**，不是现扫一遍。
    ///
    /// 于是 `/undo` 回滚了目标的 `Status` 之后这个答案**自动**跟着回来：它是图上
    /// 的一个值，不是某处维护出来的判断（同 `tools_converged` 的形状与理由）。
    pub fn await_progress(&self, target: &AgentId, until: AwaitUntil) -> AwaitProgress {
        let id = derived_atom(
            &self.store,
            &self.sources,
            &self.derived,
            &DerivedKey::AwaitReached {
                target: target.clone(),
                until,
            },
        );
        match self.store.get(id) {
            AgentValue::Bool(true) => AwaitProgress::Reached,
            AgentValue::Bool(false) => AwaitProgress::Unreachable,
            _ => AwaitProgress::Waiting,
        }
    }

    /// 这个 agent 此刻在等谁（按 target 升序，`value::awaiting` 保证）。
    ///
    /// 非创建读（`peek`，同 `inbox_of` 的先例）：探一个不在树上的 id 答空，
    /// 不在 family 里留 atom。
    pub fn awaiting_on(&self, agent: &AgentId) -> Awaiting {
        self.peek(&AtomKey::Agent(agent.clone(), Slot::AwaitingOn))
            .map(|v| awaiting::from_value(&v))
            .unwrap_or_default()
    }

    /// 建立一条等待边：`waiter` 开始等 `target` 到达 `until`。
    ///
    /// **查环在写之前**（见模块文档）。通过之后落一条 entry，记在 `waiter` 头上
    /// ——`/undo` 撤掉建立它的那一轮，这条边跟着消失。
    ///
    /// 同一条边重复建立是幂等的（`value::awaiting::to_value` 去重），不落新 entry。
    pub fn await_agent(
        &mut self,
        waiter: &AgentId,
        target: &AgentId,
        until: AwaitUntil,
    ) -> Result<(), AwaitDenied> {
        if waiter == target {
            return Err(AwaitDenied::Yourself {
                agent: waiter.clone(),
            });
        }
        if !self.in_session(target) {
            return Err(AwaitDenied::NotInSession {
                target: target.clone(),
            });
        }
        if !self.is_live(target) {
            return Err(AwaitDenied::NotLive {
                target: target.clone(),
            });
        }
        if let Some(chain) = self.path_back_to(target, waiter) {
            return Err(AwaitDenied::WouldCycle { chain });
        }

        let mut edges = self.awaiting_on(waiter);
        if edges.iter().any(|(t, u)| t == target && *u == until) {
            return Ok(());
        }
        edges.push((target.clone(), until));
        let key = AtomKey::Agent(waiter.clone(), Slot::AwaitingOn);
        let next = awaiting::to_value(edges);
        self.commit_as(waiter, "await_agent", |txn| txn.set_key(key, next));
        Ok(())
    }

    /// 撤掉 `waiter` 等 `target` 的那条（或那几条）边。等到了、或者等待方这一轮
    /// 收尾时由运行时调。
    ///
    /// 没有这条边就什么都不做，**不落 entry**（同 `set_note` 删一个不存在的 key
    /// 那条理由：幂等，重复调不该报错）。
    pub fn stop_awaiting(&mut self, waiter: &AgentId, target: &AgentId) {
        let edges = self.awaiting_on(waiter);
        let kept: Awaiting = edges
            .iter()
            .filter(|(t, _)| t != target)
            .cloned()
            .collect();
        if kept.len() == edges.len() {
            return;
        }
        let key = AtomKey::Agent(waiter.clone(), Slot::AwaitingOn);
        let next = awaiting::to_value(kept);
        self.commit_as(waiter, "await_agent", |txn| txn.set_key(key, next));
    }

    /// 从 `from` 出发顺着等待边走，**走得到 `goal` 就返回那条链**（含两端）。
    ///
    /// 这就是查环：调用方问的是「目标（或它在等的人、或它在等的人在等的人……）
    /// 是不是最终在等我」。走得到 = 加上「我等目标」这条新边就成环。
    ///
    /// 深度优先 + `seen` 去重：等待图可能本来就有分叉（一个 agent 同时等两个），
    /// 而**它本身保证无环**（每一条新边都过这道闸），所以这个遍历必然终止；
    /// `seen` 是防御性的——万一哪天有别的写入点绕过这里，也不至于死循环。
    fn path_back_to(&self, from: &AgentId, goal: &AgentId) -> Option<Vec<AgentId>> {
        let mut stack = vec![(from.clone(), vec![from.clone()])];
        let mut seen: Vec<AgentId> = Vec::new();
        while let Some((at, chain)) = stack.pop() {
            if &at == goal {
                return Some(chain);
            }
            if seen.contains(&at) {
                continue;
            }
            seen.push(at.clone());
            for (next, _) in self.awaiting_on(&at) {
                let mut chain = chain.clone();
                chain.push(next.clone());
                stack.push((next, chain));
            }
        }
        None
    }
}
