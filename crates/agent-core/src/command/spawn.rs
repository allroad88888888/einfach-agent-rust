//! [`Session::spawn_child`]：在同一棵树上长出一个子 agent。
//!
//! 决策 20（issue 006 拍板）：**子 agent 由模型经内置工具 spawn**，spawn 即一次
//! tool call，记账走既有的 command 层。这个文件是那条决策在状态侧的落点——029 的
//! `spawn_agent` 工具把模型给的入参翻译成 [`ChildConfig`] 之后调这里。
//!
//! ## 三条硬性形状
//!
//! 1. **与 root 同一条 `build_agent`**（019 的硬约束）：子 agent 的槽位不是「另
//!    一套建法」，是同一个构图函数换一个 `AgentId`。分成两条路的那一刻，undo 路径
//!    重建出来的 atom 就会和正常创建出来的不一样——而那条路径只有「长会话 + 逐出
//!    + undo」三件事同时发生才走得到，通常是在线上。
//! 2. **记账落在父 agent 的那条 `Entry` 上**：子的初始槽位值就是 spawn 这个 batch
//!    的 `changes`。于是「撤一轮连带子树」不需要任何额外机制——同一条日志、同一个
//!    `turn_id`（决策 5：turn_id 只在 root 铸，子 agent 继承）。
//! 3. **上限超了返回错误值，不 panic**（决策 20 的成本兜底）：深度 ≤3、子数 ≤8
//!    都是**参数**不是分支（红线 12 禁分支不禁参数），`AgentLimits` 可配。029 把
//!    这里的 `Err` 翻成 `is_error` 的 tool_result 喂回模型，让它自己收敛（003 的
//!    哲学：让模型看到全貌）。
//!
//! ## 号不复用
//!
//! 新 agent 的序号取「这个父 agent 的 family 键空间里出现过的最大号 + 1」，
//! **活的和墓碑一起数**。于是同一个会话里一个 `AgentId` 只属于一个 agent 的一生：
//! 复用了的话，审计时间线上会出现两个同名 agent，日志读者分不出哪段是谁的，
//! 而 undo 日志的键正是这个 id。despawn 刻意留下 `ToolsAllowed` 墓碑，
//! 为的就是让这个「最大号」在逐出之后仍然单调（见 `despawn.rs`）。

use std::sync::Arc;

use crate::graph::{AtomKey, Slot, build_agent};
use crate::ids::AgentId;
use crate::value::str_set;

use super::child_config::ChildConfig;
use super::session::Session;

/// 深度上限的默认值（决策 20）。root 是深度 0，所以 3 表示最深到 `root/a1/a2/a3`。
pub const DEFAULT_MAX_AGENT_DEPTH: usize = 3;

/// 每个 agent 的**活着的**直接子 agent 数上限的默认值（决策 20）。
///
/// 数的是活的：despawn 掉一个就空出一格。它是并发宽度的闸，不是一生的配额——
/// 「一共能 spawn 多少个」由子树轮预算管（029），两件事不该合并成一个数。
pub const DEFAULT_MAX_CHILDREN: usize = 8;

/// 子 agent 树的结构性硬限。**数字参数，不是分支**（红线 12）。
///
/// 不进原子图、不进 undo log：它是这个会话的**配置**，跟 `History` 的 cap 同一类
/// ——「用户把上限调大了」不是一次可以撤销的状态变更，撤回去只会让一批已经存在的
/// 子 agent 变成非法。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AgentLimits {
    /// `AgentId::depth()` 的上限（root = 0）。
    pub max_depth: usize,
    /// 每个 agent 活着的直接子 agent 数上限。
    pub max_children: usize,
}

impl Default for AgentLimits {
    fn default() -> Self {
        AgentLimits {
            max_depth: DEFAULT_MAX_AGENT_DEPTH,
            max_children: DEFAULT_MAX_CHILDREN,
        }
    }
}

/// spawn 被拒的理由。**全部是可预期的拒绝**，不是 bug——029 把它翻成 `is_error`
/// 的 tool_result 喂回模型。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SpawnRefused {
    /// 这个 id 不在本会话这棵树上（跨 root 不共享 store）。
    NotInSession { parent: AgentId },
    /// 父 agent 不在活名单上：从没 spawn 过、spawn 被 undo 撤了、或者已经 despawn。
    /// 给一个死掉的父 agent 挂孩子，孩子一出生就是孤儿。
    ParentNotLive { parent: AgentId },
    /// 深度撞顶（决策 20）。`depth` 是**子** agent 会落在的深度。
    DepthExceeded { depth: usize, max: usize },
    /// 子数撞顶（决策 20）。`live` 是撞顶时父 agent 已有的活子数。
    TooManyChildren { live: usize, max: usize },
}

impl Session {
    /// 当前的结构性硬限。
    pub fn agent_limits(&self) -> AgentLimits {
        self.limits
    }

    /// 改结构性硬限（决策 20 的「数字参数」）。
    ///
    /// **不追溯**：调小之后已经存在的子 agent 不会被清理，只是再 spawn 会被拒。
    /// 追溯清理等于让一次配置变更悄悄 despawn 一批正在干活的 agent，那是 despawn
    /// 该做的事，而 despawn 是一条显式命令。
    pub fn set_agent_limits(&mut self, limits: AgentLimits) {
        self.limits = limits;
    }

    /// 在 `parent` 底下长一个子 agent，返回它的 [`AgentId`]。
    ///
    /// # 做了什么
    ///
    /// 1. 两道闸（深度 / 子数）——超了返回 [`SpawnRefused`]，**不 panic**；
    /// 2. 铸一个不复用的号（见模块文档），`build_agent` 建这个 agent 的整张图
    ///    （整份 `Slot::ALL` + 一个 derived，与 root 同一条路径）；
    /// 3. 一条 `Entry`：原子写入工具授权、可选前缀授予名单、已解析 profile 与
    ///    可选重试上限。
    ///
    /// `prefix_allowed` 是 144（决策 28 的 core 半边）追加的参数，跟
    /// `config.tools_allowed` 同一个形状、不同的家：前者是 `ChildConfig` 的
    /// 一个字段，后者留在参数列表末尾——**不并进 `ChildConfig`**，因为 145 的
    /// 组料过滤要靠调用方在每个 call site 显式回答「这个子给不给开局产物」，
    /// 塞进带 `#[derive(Default)]` 的配置结构会让漏传变成静默的「什么都不给」
    /// 而不是编译期就看得见的缺项。`Some` 排序去重后落 [`Slot::PrefixAllowed`]，
    /// `None` 落 `Null`（= 不设限 = 全带，见该槽位文档）。
    ///
    /// # 为什么默认配置的 `changes` 里只有一个槽位
    ///
    /// 其余槽位此刻**就是**它们的默认值，`record_set` 不给没变的值落 `Change`
    /// （009 的「幽灵步不落条目」）。这不是记漏了：undo 这一条的语义是「回到 spawn
    /// 之前」，而 spawn 之前它们本来就是默认值。子 agent 在这一轮里后来写的东西
    /// 各有各的 entry，`undo_turn` 一并退掉。`prefix_allowed` 传 `None` 时同一条
    /// 纪律成立：落的 `Null` 就是 `Slot::PrefixAllowed` 的默认值，不产生新 `Change`
    /// ——这正是「既有调用点全传 `None`，行为零变化」在这里的落点。
    ///
    /// 029 给子 agent 播种任务消息时，那次写入落在**同一个 batch** 里，于是它自然
    /// 出现在这条 `Entry` 的 `changes` 中——机制不用改。
    ///
    /// # 建图不是状态变更
    ///
    /// `build_agent` 在 `commit_as` **之外**调用，跟 `Session::new` 给 root 建图
    /// 一样不落任何 `Entry`：建出来的槽位持的是默认值，undo 回到「这个 agent 的
    /// atom 还不存在」既没有意义也没有目标——019 早就定了「重建保证 atom 回来，
    /// 不保证值回来」，值才是日志的事。
    pub fn spawn_child(
        &mut self,
        parent: &AgentId,
        config: ChildConfig,
        prefix_allowed: Option<Vec<Arc<str>>>,
    ) -> Result<AgentId, SpawnRefused> {
        if !self.in_session(parent) {
            return Err(SpawnRefused::NotInSession {
                parent: parent.clone(),
            });
        }
        if !self.is_live(parent) {
            return Err(SpawnRefused::ParentNotLive {
                parent: parent.clone(),
            });
        }
        let depth = parent.depth() + 1;
        if depth > self.limits.max_depth {
            return Err(SpawnRefused::DepthExceeded {
                depth,
                max: self.limits.max_depth,
            });
        }
        let live = self.children_of(parent).len();
        if live >= self.limits.max_children {
            return Err(SpawnRefused::TooManyChildren {
                live,
                max: self.limits.max_children,
            });
        }

        let child = parent.child(self.next_child_seq(parent));
        build_agent(&self.store, &self.sources, &self.derived, &child);

        // 排序去重后落盘（红线 11），机制在 `value::str_set`——跟 039 激活的 skill
        // 集是同一个「有序字符串集当值」的形状，只有一处编解码。
        let tools = str_set::to_value(config.tools_allowed);
        // 同一个「有序字符串集当值」的形状（144，`Slot::PrefixAllowed` 文档）：
        // `Some` 排序去重落值，`None` 落 `Null`（= 不设限）——跟 `tools` 那一行
        // 唯一的差别是 `None` 时不经过 `str_set::to_value`，因为「不设限」不是
        // 「空集」，两者不能被编码塌成同一个值。
        let prefix_allowed_value = match prefix_allowed {
            Some(items) => str_set::to_value(items),
            None => crate::value::atom_value::AgentValue::Null,
        };
        let profile = config
            .execution_profile
            .map(|id| crate::value::atom_value::AgentValue::Text(id.into_inner()))
            .unwrap_or(crate::value::atom_value::AgentValue::Null);
        let tools_key = AtomKey::Agent(child.clone(), Slot::ToolsAllowed);
        let prefix_allowed_key = AtomKey::Agent(child.clone(), Slot::PrefixAllowed);
        let profile_key = AtomKey::Agent(child.clone(), Slot::ExecutionProfile);
        let retries_key = AtomKey::Agent(child.clone(), Slot::MaxRetries);
        self.commit_as(parent, "spawn_child", |txn| {
            txn.set_key(tools_key, tools);
            txn.set_key(prefix_allowed_key, prefix_allowed_value);
            txn.set_key(profile_key, profile);
            if let Some(max_retries) = config.max_retries {
                txn.set_key(
                    retries_key,
                    crate::value::atom_value::AgentValue::U64(max_retries as u64),
                );
            }
        });

        Ok(child)
    }

    /// 下一个还没被用过的子号。见模块文档：**活的和墓碑一起数**，所以它在整个
    /// 会话生命周期里单调递增，despawn + 再 spawn 不会撞上老 id。
    fn next_child_seq(&self, parent: &AgentId) -> u32 {
        let used = self
            .known_agents()
            .into_iter()
            .filter(|a| a.parent().as_ref() == Some(parent))
            .filter_map(|a| child_seq(parent, &a))
            .max();
        used.map_or(1, |n| n + 1)
    }
}

/// 从子 id 上读回它的序号（`root/a1` → `1`）。
///
/// 读不出来（外来的、手写的 id 段）就是 `None`——**不猜**。铸号只跳过认得出的号，
/// 认不出的段本来也不会被 [`AgentId::child`] 再造出来一次。
fn child_seq(parent: &AgentId, child: &AgentId) -> Option<u32> {
    let tail = child.as_str().strip_prefix(parent.as_str())?;
    tail.strip_prefix('/')?.strip_prefix('a')?.parse().ok()
}

/// 单测拆到独立文件（144 加了四条 `prefix_allowed` 白盒测试后本文件顶破 300 行
/// ——同 `despawn.rs`/`despawn_tests.rs` 的先例：实现与它的测试是两件事，只是
/// 测试需要 `super::*` 才能碰到 `pub(crate)`/私有项，`#[path]` 把两者接回同一个
/// 编译单元）。
#[cfg(test)]
#[path = "spawn_tests.rs"]
mod tests;
