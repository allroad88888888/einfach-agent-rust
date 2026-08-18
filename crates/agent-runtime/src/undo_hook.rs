//! 201：**还原函数这一侧的账本**——工具交回来的那个闭包住在哪、按什么键记、
//! undo 路上怎么被问到、什么时候被丢掉（决策 199 §一 §二 §九）。
//!
//! 「带着这张表调一次 undo」是另一件事，住 [`crate::undo`]。
//!
//! # 为什么住 runtime 而不是 core（红线 7）
//!
//! 还原函数会碰外部世界（删一个文件、撤一份 CRM 草稿），而 `agent-core` 不做 IO、
//! 不能持有那种闭包。core 那一侧只记「这条 entry 属于哪一档」（`Undoability` 三态，
//! 200 落的），真正的函数体在这张表里；undo 时 core 递一个回调过来问，
//! 答案由这里给（`agent_core::Session::undo_turn_with`）。
//!
//! **活句柄住 store 外**（红线 3）：形状照抄在飞 provider 凭据表与 `McpRegistry`
//! ——一个不进任何 atom、不落任何盘的进程内 map。
//!
//! # 键是 `Entry::seq`，不是 `ToolCallId`
//!
//! 199 §九 算过这笔账：`EntryMeta` 今天没有 `call_id`，加一个就要动落盘 schema；
//! 而 `seq` 由 `History` 铸造、严格递增、本来就在 `Entry` 上。undo 的回调收的正是
//! `&AgentEntry`，一次 `entry.seq` 就查到。
//!
//! 代价是**登记要分两步**：执行体交出函数的那一刻 entry 还没落地（tool_result
//! 事件要等泵下一圈 `session.step` 才变成一条 entry），所以先按 `call_id` 暂存
//! （[`UndoHooks::stage`]），等那条 entry 真的落地了再挪到 `seq` 上
//! （[`settle`]）。这跟 `Session::mark_no_undo` 今天「派发时登记 call_id、
//! 结果落地时翻译成 entry 上的位」是同一条路的镜像，**不新发明记账**。
//!
//! # 跑过就不再跑第二次，但也不假装「函数不见了」
//!
//! [`UndoFn`] 是 `FnOnce`：还原只跑一次（论文 §5.1.1 的 `armed` 标志防的就是这个，
//! 我们用类型防）。可「跑过了」不等于「没有过」——跑完就把键删掉的话，下一次
//! 有人问同一条 entry 会拿到 [`HookOutcome::Lost`]，而那句话的措辞是「**还原函数
//! 随进程重启消失了**」，对一个刚刚在本进程里跑过的钩子是**假话**。
//!
//! 所以跑完留一块墓碑（[`Hook::Spent`]）：
//!
//! | 再问一次 | 答什么 | 为什么 |
//! |---|---|---|
//! | 上次跑成了 | `Ok` | 外部世界已经收拾干净了，这次只要退状态。**redo 之后再 undo 走的就是这条**——redo 不重放副作用（200 §5），所以那个文件本来就还没回来 |
//! | 上次跑挂了 | `Failed(同一句原因)` | `FnOnce` 已经被消费，没有第二次机会。措辞逐字不变，用户看到的还是「跑了但失败了」，不会变成「函数不见了」 |

use std::collections::BTreeMap;
use std::sync::Arc;

use agent_core::{Event, HookOutcome, Session, ToolCallId, Undoability};

use crate::ctx::RunnerCtx;

/// 一次调用的**逆**：跑一次，把这次调用在外部世界留下的东西收拾掉。
///
/// `FnOnce`：还原只跑一次，跑第二次等于「在一个没有对应应用的状态上跑一个逆」。
/// `Send + Sync`：它跟着 `RunnerCtx` 住在 actor 线程上，要能被移过去。
///
/// **捕获执行时的现场正是设计意图**（199 §一：逆是在执行的那个状态上选的）——
/// 旧文件内容、创建出来的资源 id 都该捕进来。**但不能捕获 `&Session`**：它跑在
/// undo 路上，那时 core 正在回滚状态，让它同时写状态就是在一次回滚中间插一次前向
/// 写入，红线 6 的账当场乱掉。状态那半边由 journal 的回滚承担，还原函数只管外部
/// 世界——生命周期也不允许（这个类型是 `'static` 的）。
pub type UndoFn = Box<dyn FnOnce() -> Result<(), Arc<str>> + Send + Sync>;

/// 一次调用在**外部世界**留下了什么，以及怎么收拾（决策 199 §一）。
///
/// **三态，不是 `Option<UndoFn>`**：`Option` 会把「没碰外部世界」（`fs/read`）和
/// 「碰了但撤不回」（`shell/exec`）压成同一个 `None`，而落盘那一位是三态
/// （`agent_core::Undoability`）。返回类型必须与它同构，否则装配那一步只能靠猜，
/// 猜错的方向是「把纯读也变成屏障」或者「把不可逆的静默放过」。
///
/// 一一对应（翻译在 [`crate::session_tool_ext`] 做，两个类型**不合并**：这个是
/// 工具交代的事实，那个是这条 entry 的记账，core 不认识 `UndoFn`）：
///
/// | `Aftermath` | `Undoability` | 谁 |
/// |---|---|---|
/// | [`Aftermath::Nothing`] | `StateOnly` | 纯读、只写状态的工具 |
/// | [`Aftermath::Undo`] | `Hooked` | 碰了外部世界、交回了逆 |
/// | [`Aftermath::Irreversible`] | `Blocked` | 碰了，交不出逆 —— 屏障 |
pub enum Aftermath {
    /// 没碰外部世界——状态回滚就够了。
    Nothing,
    /// 碰了，这是还原它的函数。
    Undo(UndoFn),
    /// 碰了，还不回去——撤销撞上它停下来问（就是今天的屏障）。
    Irreversible,
}

/// 表里的一格。函数跑过之后**不删键、改成墓碑**，理由见模块文档最后一节。
enum Hook {
    /// 还没跑过。
    Ready(UndoFn),
    /// 跑过了：`None` = 成功，`Some(原因)` = 失败。
    Spent(Option<Arc<str>>),
}

/// `seq → 还原函数` 的账本 + 一格「entry 还没落地」的暂存区。
///
/// 住在 [`RunnerCtx`] 上（会话级资源，跟在飞凭据表同一类）。**不进 store、不落盘**
/// ——闭包不跨进程，这正是 199 §九 要 `Undoability` 三态的原因。
#[derive(Default)]
pub(crate) struct UndoHooks {
    /// 执行体已经交回、但对应的 entry 还没落地的那些函数，按 `call_id` 暂存。
    ///
    /// `Vec` 不是 map：同时在这里的最多是**一批工具调用**（模型一次响应里那几个
    /// `ToolUse`），线性找比一棵树便宜，也不需要额外的有序容器。
    staged: Vec<(ToolCallId, UndoFn)>,
    /// 已经挂到 entry 上的那些。键是 `Entry::seq`。
    table: BTreeMap<u64, Hook>,
}

impl UndoHooks {
    /// 执行体交回了一个还原函数，但 entry 还没落地——先按 `call_id` 存着。
    ///
    /// 同一个 `call_id` 交两次（不该发生：一次调用只落地一次）**后说的算**，
    /// 跟 `Session::mark_tool` 的幂等规则一致。
    pub(crate) fn stage(&mut self, call_id: ToolCallId, undo: UndoFn) {
        self.staged.retain(|(id, _)| id != &call_id);
        self.staged.push((call_id, undo));
    }

    /// 这一轮被取消，暂存区里那些函数等的事件永远不会被 step 了——丢掉。
    ///
    /// 不丢的话它们会一直占着内存等一条不会来的 entry。**丢掉是对的不是漏的**：
    /// 那次调用的结果被 epoch 闸/取消正当地作废了，日志里根本没有对应的 entry，
    /// 没有任何 `seq` 能挂上它们（199 §九 画的那条边界的另一面）。
    pub(crate) fn discard_staged(&mut self) {
        self.staged.clear();
    }

    /// 暂存区是空的吗——泵拿它决定要不要为这一步多做一次 `call_id` 判定。
    pub(crate) fn nothing_staged(&self) -> bool {
        self.staged.is_empty()
    }

    /// undo 路上被问到一条 `Hooked` 的 entry：跑它的钩子。
    ///
    /// core 只会为 [`Undoability::Hooked`] 的 entry 调到这里（`undo_hook::obstacle`），
    /// 所以「表里查不到」在这里是一句确定的话：**说好有函数、函数没了**，
    /// 也就是进程重启过（200 的 [`HookOutcome::Lost`]）。
    pub(crate) fn run(&mut self, seq: u64) -> HookOutcome {
        let Some(hook) = self.table.remove(&seq) else {
            return HookOutcome::Lost;
        };
        let (spent, outcome) = match hook {
            Hook::Ready(undo) => match undo() {
                Ok(()) => (None, HookOutcome::Ok),
                Err(why) => (Some(Arc::clone(&why)), HookOutcome::Failed(why)),
            },
            // 已经跑过一次了。`FnOnce` 没有第二次，但答案要跟第一次逐字一致。
            Hook::Spent(None) => (None, HookOutcome::Ok),
            Hook::Spent(Some(why)) => (Some(Arc::clone(&why)), HookOutcome::Failed(why)),
        };
        self.table.insert(seq, Hook::Spent(spent));
        outcome
    }

    /// 日志被 cap 挤掉的那些 entry，对应的钩子也该丢。
    ///
    /// **显式清，不许靠「反正有界」**（201 原文）：`DEFAULT_HISTORY_CAP` 是 100，
    /// 所以不清也涨不到天上去——但「涨不到天上去」和「清干净了」是两句话，而且
    /// cap 是宿主可配的（`Session::set_history_cap(None)` 就是无上限）。
    ///
    /// 判据是**日志里最老那条的 `seq`**：比它还老的 entry 已经不可能被 undo 到，
    /// 挂在它们身上的函数从此没有任何调用路径。`split_off` 一刀切在有序键上，
    /// 不逐个比。
    pub(crate) fn prune(&mut self, session: &Session) {
        let Some(oldest) = session.history().entries().next().map(|entry| entry.seq) else {
            self.table.clear();
            return;
        };
        self.table = self.table.split_off(&oldest);
    }

    /// 测试用：表里现在挂着几条。
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.table.len()
    }
}

/// 这一步 step 完之后要不要去接一个暂存的钩子：接哪一次调用的、以及 step 之前
/// 日志的末端在哪。
///
/// `seq_before` 是**「这一步真的落了一条新 entry 吗」**的判据。少了它就得靠
/// 「最后一条是不是 `Hooked`」来猜，而那句话在「这次结果被 epoch 闸挡掉了、
/// 一条 entry 都没落」时会指向**上一次**某条 `Hooked` 的 entry，把函数挂到别人
/// 身上——不报错，只是 undo 那一刻跑了个不属于它的逆。
pub(crate) struct Landing {
    call_id: ToolCallId,
    seq_before: Option<u64>,
}

/// step **之前**问一次：这条事件会不会让某个暂存的钩子落地。
///
/// 暂存区空（绝大多数会话的绝大多数步）时直接 `None`，一次 clone 都不做。
pub(crate) fn landing(ctx: &RunnerCtx, session: &Session, event: &Event) -> Option<Landing> {
    if ctx.undo_hooks.nothing_staged() {
        return None;
    }
    // 只有这两条事件会让一次工具调用的结果变成一条 entry（`tool_result` /
    // `tool_failed` 两个转移，两者都会走 `mark_tool_undoability`）。
    let call_id = match event {
        Event::ToolResult { call_id, .. } | Event::ToolFailed { call_id, .. } => call_id.clone(),
        _ => return None,
    };
    Some(Landing {
        call_id,
        seq_before: session.last_entry().map(|entry| entry.seq),
    })
}

/// step **之后**：把暂存的那个函数挪到刚落地那条 entry 的 `seq` 上。
///
/// 三道判据，任何一道不成立就把函数**丢掉**（而不是硬挂上去）：
///
/// 1. 暂存区里有这个 `call_id`；
/// 2. 这一步真的落了一条新 entry（`seq` 前进了）；
/// 3. 那条 entry 的档位真是 `Hooked`（也就是 `mark_hooked` 那一位确实翻译上去了）。
///
/// 第 2、3 条不成立 = 这次结果没能进日志（epoch 闸挡掉 / 转移表判了协议违规）。
/// 那时**没有任何 entry 代表这次调用**，undo 也就永远走不到它——留着函数只会
/// 让它在下一次 cap 清理前占着内存，而挂到别的 seq 上是实打实的错值。
pub(crate) fn settle(ctx: &mut RunnerCtx, session: &Session, landing: Option<Landing>) {
    let Some(Landing {
        call_id,
        seq_before,
    }) = landing
    else {
        return;
    };
    let hooks = &mut ctx.undo_hooks;
    let Some(at) = hooks.staged.iter().position(|(id, _)| id == &call_id) else {
        return;
    };
    let (_, undo) = hooks.staged.remove(at);
    let Some(entry) = session.last_entry() else {
        return;
    };
    if Some(entry.seq) == seq_before || entry.meta.undoability != Undoability::Hooked {
        return;
    }
    hooks.table.insert(entry.seq, Hook::Ready(undo));
    hooks.prune(session);
}

#[cfg(test)]
#[path = "undo_hook_tests.rs"]
mod tests;
