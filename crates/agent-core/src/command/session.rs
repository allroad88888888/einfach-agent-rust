//! [`Session`] 本体：一个 root agent + 它的整棵子树 = 一个 session = 一个 actor
//! 线程 = 一个 store（STATE-MODEL §「并发」）。
//!
//! 这个文件只放**结构与会话级命令**：怎么开一个 session、怎么开新一轮、怎么调
//! 预算、怎么标记不可逆调用。读口在 [`read`](super::read)，一次转移怎么落成一条
//! `Entry` 在 [`commit`](super::commit)，转移表本体在 `transitions/`，
//! undo/redo 在 [`undo`](super::undo)。
//!
//! ## 不暴露 store 本体
//!
//! `Session` 没有 `store()` 访问器，这是红线 2 的结构面：拿得到 `Store` 就拿得到
//! 裸 `set`，那次写入不进 undo log，undo 越过它时这个 atom 停在新值上、其余全部
//! 回滚——「测试全过、线上偶发」的自相矛盾。宿主要的东西一律由读口给出（值的
//! 克隆），要写就走命令。

use std::cell::RefCell;
use std::rc::Rc;

use agent_store::{AtomFamily, History, Store};

use crate::engine::epoch::Epoch;
use crate::engine::state::TurnStatus;
use crate::graph::{AgentStore, DerivedFamily, SourceFamily, build_agent};
use crate::ids::{AgentId, ToolCallId};

use super::meta::{AgentHistory, Undoability};
use super::spawn::AgentLimits;

/// 日志上限（STATE-MODEL §「cap 与分支」：默认 100 条，溢出从最老一端丢）。
///
/// 常量在**会话层**而不是 `History` 里：`History` 对「一个会话该有多大」一无所知，
/// 就像它对 `turn_id` 一无所知一样（018 的裁决）。宿主要别的值就调
/// [`Session::set_history_cap`]。
pub const DEFAULT_HISTORY_CAP: usize = 100;

/// 一个会话的全部状态 + 它的 command log。
///
/// **完整状态 = 所有 primitive atom 的值**（[`Session::primitives`](super::read)），
/// derived 全部可重算。于是快照 = 序列化所有 primitive、恢复 = 重建 atom 图 + 灌回
/// primitive 值 + derived 自动重算、undo 记录量正比于源状态而不是全部状态。
///
/// # 不住在原子图里的三样东西，以及为什么
///
/// | 字段 | 为什么不是 atom |
/// |------|----------------|
/// | `epoch` | 世代**只增不减**。进了原子图就会被 undo 回滚回去，而 undo 恰恰是要 bump 它的那个动作（红线 6 会自相矛盾）。它进 `EntryMeta`，恢复时取日志里的最大值继续发 |
/// | `turn_id` | 同上，且它是**日志的分组依据**，不是被日志记录的状态 |
/// | `tool_marks` | 宿主在派发工具时告诉 core「这次调用撤起来算哪一档」（199 §九的三态）。真正的家是 `ToolCall(_, _, Request)` 那个发起时快照，但 core 没有工具表，`AgentValue` 也没有装它的变体——现在编一个占位快照就是 002 裁决过的那种编造（假的 `Irreversible` 会让 undo 白拦一次 `fs/read`）。M2 先做成运行时提示，027 接线时由 CLI 的工具表喂 |
/// | `limits` | 子 agent 树的结构性硬限（深度 / 子数，决策 20）。是这个会话的**配置**，不是它的状态：调大上限不是一次可以撤销的状态变更，撤回去只会让一批已经存在的子 agent 变成非法。跟 `History` 的 cap 同一类 |
///
/// # `agent` 字段是 root，不是「当前 agent」
///
/// 028 起一个 `Session` 装的是**整棵树**（决策 3）。这个字段只回答「这棵树的根是
/// 谁」——会话级命令（`begin_turn` / `set_max_*`）落在它头上，`turn_id` 由它铸
/// （决策 5）。「这一步替谁做」是每次 `commit_as` 的参数，从事件的 `agent` 字段
/// 路由过来。
pub struct Session {
    pub(super) agent: AgentId,
    pub(super) store: AgentStore,
    pub(super) sources: SourceFamily,
    pub(super) derived: DerivedFamily,
    pub(super) history: AgentHistory,
    pub(super) epoch: Epoch,
    pub(super) turn_id: u64,
    pub(super) tool_marks: Vec<(ToolCallId, Undoability)>,
    pub(super) limits: AgentLimits,
}

impl Session {
    /// 开一个会话：给 root 建图（整份 `Slot::ALL` + 一个 derived，见
    /// `graph::build_agent`）、空日志、[`Epoch::START`]、`turn_id` 从 1 起、
    /// 子 agent 上限取默认值（[`AgentLimits::default`]）。
    ///
    /// 建图之后**一条 `Entry` 都没有**：建 atom 不是状态变更，落的是每个槽位的默认值
    /// （`Slot::default_value()`），undo 回到「会话开始之前」没有意义也没有目标。
    ///
    /// **root 不走特殊路径**：`spawn_child` 给子 agent 建图调的是同一个
    /// `build_agent`（019 的硬约束）。
    pub fn new(agent: AgentId) -> Self {
        let store: AgentStore = Store::new();
        let sources: SourceFamily = Rc::new(RefCell::new(AtomFamily::new()));
        let derived: DerivedFamily = Rc::new(RefCell::new(AtomFamily::new()));
        build_agent(&store, &sources, &derived, &agent);

        let mut history = History::new();
        history.set_cap(Some(DEFAULT_HISTORY_CAP));

        Session {
            agent,
            store,
            sources,
            derived,
            history,
            epoch: Epoch::START,
            turn_id: 1,
            tool_marks: Vec::new(),
            limits: AgentLimits::default(),
        }
    }

    /// 开新一轮：`turn_id` 前进一格，状态回到 `Idle`，工具槽清空，本轮已用的
    /// 轮数/重试计数清零。消息历史、前缀镜像、消息号计数器、两个上限全部延续。
    ///
    /// **必须显式调用，`Done`/`Failed` 收到 `UserInput` 仍然是协议违规**：turn 边界是
    /// `undo_turn` 的分组依据，藏进一格转移里就意味着「一轮从哪开始」这个会话层面的
    /// 概念被塞进了转移表（002/016 已经裁决过它不属于那里）。M1 的宿主本来也是显式
    /// 调 `agent_cli::next_turn` 开新一轮的，这里是同一件事换了个住处。
    ///
    /// `turn_id` **只增不减**：undo 掉一整轮之后再开新一轮，拿到的是一个新号，不是
    /// 被退掉的那个号。日志里「turn 7」在整条会话生命周期里永远指同一轮，审计回放
    /// 才对得上（跟 `seq` 不回收是同一条理由）。
    ///
    /// 上一轮被取消时 M1 的宿主还会丢弃那一轮新增的消息（`agent_cli::next_turn` 的
    /// `messages_before`）——**这里不做**：那是宿主的显示策略，而 M2 里「把这一轮
    /// 抹掉」有了正牌答案，就是 [`undo_turn`](Session::undo_turn)。
    ///
    /// # 只有 root 开新一轮（决策 5）
    ///
    /// 铸号点就收在这里，子 agent 没有对应的命令：它们的 entry 继承所在 root turn
    /// 的 `turn_id`、不产生新的 turn 边界，于是 `undo_turn` 一次退回一整个 root
    /// turn，连带那一轮里所有子 agent 的工作。子 agent 的**轮状态**（status /
    /// 工具槽 / 预算）另有出生方式：`spawn_child` 建图时它们就是默认值，
    /// 那正好等价于「刚开一轮」。
    ///
    /// 这一格写的也只是 root 自己的三个槽位——子 agent 的状态不被 root 的新一轮
    /// 清掉。跨 root turn 还活着的子 agent 是 029 才会出现的形态，那时它继续用
    /// 自己的槽位，不需要在这里被谁重置。
    pub fn begin_turn(&mut self) {
        self.turn_id += 1;
        self.commit("begin_turn", |txn| {
            txn.set_status(TurnStatus::Idle);
            txn.set_tool_slots(Vec::new());
            txn.clear_turn_budget();
        });
    }

    /// 覆盖 `max_turns`（016 的宿主可配上限）。走命令 → 进日志 → 可回滚，
    /// 跟别的 primitive 一视同仁。
    pub fn set_max_turns(&mut self, max_turns: u32) {
        self.commit("set_max_turns", |txn| txn.set_max_turns(max_turns));
    }

    /// 覆盖 `max_retries`。
    pub fn set_max_retries(&mut self, max_retries: u32) {
        self.commit("set_max_retries", |txn| txn.set_max_retries(max_retries));
    }

    /// 告诉 core「这次工具调用**没有**交回还原函数」→ [`Undoability::Blocked`]。
    ///
    /// 宿主在**派发工具时**调用——它持有工具表、它才知道执行完交没交回还原函数，
    /// core 没有。落到日志上的效果是：记录这次调用结果的那条 `Entry` 是屏障，
    /// `undo_turn` 走到它会返回
    /// [`UndoReport::Blocked`](super::UndoReport::Blocked) 而不是静默回滚
    /// （`docs/TOOLS.md`：undo 越过不可逆操作要停下问）。
    pub fn mark_irreversible(&mut self, call_id: ToolCallId) {
        self.mark_tool(call_id, Undoability::Blocked);
    }

    /// 告诉 core「这次工具调用**交回了**还原函数」→ [`Undoability::Hooked`]。
    ///
    /// 决策 199 §一：可逆性从此是**每次调用**的属性，不是每个工具的属性——
    /// `fs/write` 写新文件、覆盖旧文件、写失败，同一个工具三次调用三种还原方式，
    /// 枚举表达不了，函数天然表达了。core 到头到尾不认识 `UndoFn` 这个类型
    /// （红线 7）：函数本体住 runtime 的钩子表，按 `Entry::seq` 键；这里只记
    /// 「这一条有钩子」这一位，好让 undo 路知道该去问一次。
    pub fn mark_hooked(&mut self, call_id: ToolCallId) {
        self.mark_tool(call_id, Undoability::Hooked);
    }

    /// 同一个 `call_id` 重复登记是幂等的（**后说的算**：宿主改主意只可能是因为
    /// 它拿到了更准的信息，比如工具真的跑完之后才知道有没有还原函数）。
    fn mark_tool(&mut self, call_id: ToolCallId, undoability: Undoability) {
        if let Some(slot) = self.tool_marks.iter_mut().find(|(id, _)| *id == call_id) {
            slot.1 = undoability;
            return;
        }
        self.tool_marks.push((call_id, undoability));
    }

    /// 改日志上限。`None` = 无上限。见 [`DEFAULT_HISTORY_CAP`]。
    pub fn set_history_cap(&mut self, cap: Option<usize>) {
        self.history.set_cap(cap);
    }

    /// 清掉上一次请求的前缀镜像（027：CLI 切 provider 时调，见
    /// `Txn::clear_prev_prefix` 的文档）。走命令 → 进日志 → 可回滚，
    /// 跟别的 primitive 一视同仁——`TurnState` 时代那种直接赋值
    /// `state.prev_prefix = None` 绕过了 undo log，红线 2 不允许原样搬过来。
    pub fn clear_prev_prefix(&mut self) {
        self.commit("clear_prev_prefix", |txn| txn.clear_prev_prefix());
    }

    /// 取走积累的裁剪事件（018）。宿主转发给 `SessionStore::drop_oldest` /
    /// `drop_after`（011），取走即清空；不取就一直攒着——core 不做 IO（红线 7）。
    pub fn take_drop_events(&mut self) -> Vec<agent_store::DropEvent> {
        self.history.take_drop_events()
    }
}
