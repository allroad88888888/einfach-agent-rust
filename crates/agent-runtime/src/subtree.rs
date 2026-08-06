//! 子 agent 的槽位记账：**哪个子 agent 对应父 agent 的哪个 spawn 槽**，
//! 以及子 agent 到终态之后那个槽收敛成什么。
//!
//! # 为什么结果回父是 tool_result，而不是一个 `ChildFinished` 事件
//!
//! 决策 20 / issue 006 的决策记录已经拍板：spawn 是一次 tool call，它的槽位天然
//! 走 `ToolsPending` 的收敛路径，所以「等所有子完成」不需要任何新机制——父的
//! 那几个槽位全部 `Finished` 就是「等到齐了」。001 当年推迟 `ChildFinished` 时
//! 的直觉（「未必长成一个事件」）在这里被验证为正确：这个文件把子 agent 的终态
//! 翻译成 `Event::ToolResult` / `Event::ToolFailed`，喂回去的是转移表已经有的那
//! 两格，`agent-core` 一行没加。
//!
//! # 也没有「等所有子完成」的汇聚 derived（029 §注意）
//!
//! 同一条理由：父的 spawn 槽位收敛**就是**等待语义。为它再建一个读遍所有子
//! `Status` 的 derived，等于给同一个问题准备两个可能对不上的答案——而红线 4 的
//! 孪生条款（汇聚 derived 必须按 `AgentId` 现查 family）之所以危险，正是因为
//! 那种 atom 一旦存在就会被到处引用。028 为它留的 `StillRead` 黑盒缺口
//! （despawn 撞上「仍被读依赖」那条分支的真实触发场景）因此顺延，如实记录。
//!
//! # 记的是 `AgentId` / `ToolCallId`，不是 `AtomId`（红线 4 孪生条款）
//!
//! 这张表跨越「起飞」和「落地」两个时刻，中间可能夹着 undo。存 `AtomId` 就是把
//! 一个只在进程内、只在这一版图上有效的号码缓存过一次回滚——查询一律拿
//! `AgentId` 现问 `Session`（`status_of` / `messages_of`），一次都不缓存。
//!
//! # 052：后台子 agent 的第二张表（detached）与 stash
//!
//! `background=true` 的 spawn 在 `crate::spawn_tool` 那一刻就把父的槽收敛掉了
//! （回一个 `{"agent_id":...}`），**没有任何槽位在等这个子**。于是它不能记进
//! `slots`（那张表的语义就是「谁在等谁」），另开一张 [`Detached`]；它落终态时也
//! 不该回写父（父那槽早收敛了，再回写一条就是幽灵 tool_result），结果转存进
//! 「已完成未领取」的 [`Stashed`] 一栏，等 `collect` 来领。
//!
//! 三张表**全是 `Subtree` 的局部字段**，而 `Subtree` 每次 `resume` 重建
//! （`runner.rs` 的 `Subtree::default()`）——**turn 内生死，不跨 `run_turn`**
//! （ORCHESTRATION §二 的决策：子 agent 不跨 turn）。别把它们做成 store 落地的
//! 跨 turn 映射，那是被明确延后的另一件事。
//!
//! # 053：`collect` 只是**往 `slots` 里补一笔**
//!
//! 后台子被 `collect` 领取时的两条出路都不需要新机制：已经在 stash 里的当场端走
//! （[`Subtree::take_stashed`]，领取即消费），还在跑的就记一笔
//! [`Subtree::record`]——从这一刻起它跟一个前台 spawn 出来的子**逐字一样**，
//! 由 [`Subtree::harvest_slots`] 在它落终态时回写到 collect 那个槽。
//! ORCHESTRATION §三 那句「前台 spawn ≡ spawn(bg) + 紧跟 collect」在代码上的
//! 形状，就是这两张表共用同一条收割路。

use std::sync::Arc;

use agent_core::{AgentId, Epoch, Event, Session, ToolCallId};

use crate::child_outcome::outcome;
use crate::ctx::RunnerCtx;
use crate::event::RunnerEvent;

/// 一个还没收敛的槽：`srv:agent/spawn`（前台）或 `srv:agent/collect`（053）。
struct ChildSlot {
    child: AgentId,
    parent: AgentId,
    call_id: ToolCallId,
    /// **发起那次调用的世代**，不是收敛那一刻的。父那个槽等的是这一代发出去的
    /// 那次调用；中间要是被取消/undo 推过世代，这条结果就该跟别的在飞回执一样
    /// 被 `Session::step` 的闸挡掉（红线 6）——用「现在的 epoch」交差等于绕过闸。
    ///
    /// collect 绑定记的是**那次 collect 调用**的世代（不是当初 spawn 的）：父在等
    /// 的是 collect 这次调用，`Effect::ExecuteTool` 带来的就是它。
    epoch: Epoch,
    /// 哪个工具占着这个槽。只进 [`RunnerEvent::ToolExecuted`] 通报——通报里写
    /// `spawn` 而槽位其实是 collect 的，面板上就会出现一次对不上任何调用的执行。
    tool: &'static str,
    outcome: ChildSlotOutcome,
}

/// 槽位终态的翻译策略。视觉子额外记住图片数与是否真的撞过 provider deadline，
/// 这样收割时能产出稳定的视觉信封，而不把普通子 agent 的既有文案一起改掉。
enum ChildSlotOutcome {
    Generic,
    Vision {
        images_inspected: usize,
        timed_out: bool,
    },
}

/// 一个后台（detached）子 agent：没有任何槽位在等它（052）。
pub(crate) struct Detached {
    pub(crate) child: AgentId,
    /// 谁 spawn 的它——告警/despawn 记账都挂在它名下（`despawn_child` 那条
    /// teardown entry 本来也记在父名下）。
    pub(crate) parent: AgentId,
    /// **spawn 那一刻的世代**，跟 [`ChildSlot::epoch`] 同一条理由（红线 6）：
    /// 结果进 stash 之前跟 `Session::epoch()` 比一次，不等就丢。
    epoch: Epoch,
}

/// 一份「已完成未领取」的后台子 agent 结果（052 的 stash，053 的 `collect` 领它）。
pub(crate) struct Stashed {
    pub(crate) child: AgentId,
    /// 谁 spawn 的它。**只有轮末告警用**（054）：`RunnerEvent::OrphanedChild`
    /// 的归属恒是父（没领是父的编排失误），而 `Stashed` 是那条路上唯一还记得
    /// 父是谁的地方——从 `child` 反推 `AgentId::parent()` 也行，但那会给一条
    /// 结构上不可能发生的 `None` 留一个兜底分支。
    pub(crate) parent: AgentId,
    pub(crate) content: Arc<str>,
    pub(crate) is_error: bool,
}

/// 本轮所有在飞的子 agent。
#[derive(Default)]
pub(crate) struct Subtree {
    slots: Vec<ChildSlot>,
    /// 后台子（052）：spawn 槽已经收敛，没人在等。
    detached: Vec<Detached>,
    /// 后台子已经跑完、但还没人领的结果。
    stash: Vec<Stashed>,
}

impl Subtree {
    /// 记一笔：`child` 干完了要去认领 `parent` 的 `call_id` 那个槽。
    ///
    /// `tool` 是占着这个槽的工具名（`SPAWN_TOOL` / `COLLECT_TOOL`）——由调用方给
    /// 而不是这里默认成 spawn：默认值会让 053 那条路悄悄发出一条名字对不上的
    /// 通报，而通报不参与任何判断，错了不会有任何东西变红。
    pub(crate) fn record(
        &mut self,
        child: AgentId,
        parent: AgentId,
        call_id: ToolCallId,
        epoch: Epoch,
        tool: &'static str,
    ) {
        self.slots.push(ChildSlot {
            child,
            parent,
            call_id,
            epoch,
            tool,
            outcome: ChildSlotOutcome::Generic,
        });
    }

    /// 专用视觉子占住的槽。它仍走同一棵子树的生命周期，只在最终正文翻译上使用
    /// `agent_core::vision` 的稳定信封。
    pub(crate) fn record_vision(
        &mut self,
        child: AgentId,
        parent: AgentId,
        call_id: ToolCallId,
        epoch: Epoch,
        images_inspected: usize,
    ) {
        self.slots.push(ChildSlot {
            child,
            parent,
            call_id,
            epoch,
            tool: crate::vision_tool::VISION_INSPECT_TOOL,
            outcome: ChildSlotOutcome::Vision {
                images_inspected,
                timed_out: false,
            },
        });
    }

    /// provider 截止线真实触发时留下运行时标记。core 的最终状态会把 timeout 与
    /// 其它可重试 provider 失败都归一成 `Retryable`；这一个 bit 保留稳定
    /// `vision_timeout` 所需的来源，而不把 endpoint/provider 细节写进 durable 状态。
    pub(crate) fn record_provider_timeout(&mut self, child: &AgentId) {
        let Some(slot) = self.slots.iter_mut().find(|slot| &slot.child == child) else {
            return;
        };
        if let ChildSlotOutcome::Vision { timed_out, .. } = &mut slot.outcome {
            *timed_out = true;
        }
    }

    /// A retry starts a new provider attempt. The timeout bit describes the terminal attempt,
    /// rather than any earlier attempt that core legitimately retried.
    pub(crate) fn record_provider_start(&mut self, child: &AgentId) {
        let Some(slot) = self.slots.iter_mut().find(|slot| &slot.child == child) else {
            return;
        };
        if let ChildSlotOutcome::Vision { timed_out, .. } = &mut slot.outcome {
            *timed_out = false;
        }
    }

    /// 053：这个后台子还在 detached 名单上吗（= 还在跑、还没进 stash）。
    pub(crate) fn is_detached(&self, child: &AgentId) -> bool {
        self.detached.iter().any(|entry| &entry.child == child)
    }

    /// 053：已经有槽在等这个子了吗（前台 spawn 的槽，或者上一次 collect 绑的）。
    pub(crate) fn is_awaited(&self, child: &AgentId) -> bool {
        self.slots.iter().any(|slot| &slot.child == child)
    }

    /// 053 的**领取即消费**：stash 里有这个子的结果就端走，同时从 stash 划掉。
    /// 第二次 collect 同一个 id 因此拿到 `None`——一份结果只能领一次。
    pub(crate) fn take_stashed(&mut self, child: &AgentId) -> Option<Stashed> {
        let at = self.stash.iter().position(|entry| &entry.child == child)?;
        Some(self.stash.remove(at))
    }

    /// 053 的拒绝文案用：此刻**领得动**的后代（跑完躺 stash 的 + 还在跑的后台子）。
    ///
    /// 按 `AgentId` 排序：这段文本会进下一轮 prompt（红线 11），而两张表的插入
    /// 顺序取决于「谁先落终态」这种运行期时序，不排的话同一个世界能渲染出两种
    /// 字节。理由跟 `status_tool::descendants` 自己排一次一模一样。
    pub(crate) fn collectable(&self) -> Vec<&AgentId> {
        let mut out: Vec<&AgentId> = self
            .stash
            .iter()
            .map(|entry| &entry.child)
            .chain(self.detached.iter().map(|entry| &entry.child))
            .collect();
        out.sort();
        out
    }

    /// 记一笔后台子（052）：**没有 `call_id`**——父那个 spawn 槽在 `dispatch` 里
    /// 已经当场收敛了，这张表记的只是「这个子还在跑，而且没人在等它」。
    pub(crate) fn detach(&mut self, child: AgentId, parent: AgentId, epoch: Epoch) {
        self.detached.push(Detached {
            child,
            parent,
            epoch,
        });
    }

    /// 轮末清算用（`crate::orphan`）：detached 名单里**还活着、且没有 collect
    /// 绑定**的子，连同它们从名单里一起摘走。
    ///
    /// 「没有 collect 绑定」这一条从 053 起真的会挡人：绑了 collect 的子是父正等
    /// 着的（父那个 collect 槽 `Pending` → root 非终态 → `orphan::reap` 提早返回），
    /// 而**绑定期间它一直留在 detached 名单上**——所以这条判据不是防御性的赘语，
    /// 它是「被等着的活不算孤儿」这句话本身。
    ///
    /// 已经不活的（spawn 那一轮被 undo 撤了）**不算孤儿**：没有东西要拆，也没有
    /// 什么可告警的，原样留在名单里等这一轮结束跟着 `Subtree` 一起消失。
    pub(crate) fn take_orphans(&mut self, session: &Session) -> Vec<Detached> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.detached.len() {
            let child = &self.detached[i].child;
            if self.is_awaited(child) || !session.is_live(child) {
                i += 1;
                continue;
            }
            out.push(self.detached.remove(i));
        }
        out
    }

    /// 轮末清算用（`crate::orphan`）：stash 里没人领的结果，全部取走。
    pub(crate) fn take_stash(&mut self) -> Vec<Stashed> {
        std::mem::take(&mut self.stash)
    }

    /// 收割：每个已经到终态的子 agent 各产出一条喂给**父** agent 的事件，并从
    /// 表里划掉。没到终态的原样留着。
    ///
    /// 一次收割可能同时产出多条（两个子 agent 在同一批事件里先后落终态），
    /// 顺序按记账顺序 = 模型请求 spawn 的顺序，确定。
    ///
    /// 后台子（detached）在同一次收割里走[另一条路](Subtree::harvest_detached)：
    /// 落终态时进 stash，**不产出任何喂回泵的事件**。
    pub(crate) fn harvest(&mut self, session: &Session, ctx: &mut RunnerCtx) -> Vec<Event> {
        let events = self.harvest_slots(session, ctx);
        self.harvest_detached(session);
        events
    }

    fn harvest_slots(&mut self, session: &Session, ctx: &mut RunnerCtx) -> Vec<Event> {
        let mut events = Vec::new();
        let mut i = 0;
        while i < self.slots.len() {
            let status = session.status_of(&self.slots[i].child);
            if !status.is_terminal() {
                i += 1;
                continue;
            }
            let slot = self.slots.remove(i);
            // 053：这个槽要是一次 collect，子同时还记在 detached 名单上（绑定期间
            // 它一直留在那儿，好让轮末清算认得出「有人在等它」）。结果既然已经回给
            // 父了，就把它从名单里划掉——不划的话，紧接着跑的 `harvest_detached`
            // 会看到「终态 + 没人等（槽刚被上面摘掉）」，把同一份结果**再塞进
            // stash 一次**，轮末再报一句「跑完没人领」：领了，还报。
            // 前台 spawn 的子从来不在这张名单上，这一行对它是空操作。
            self.detached.retain(|entry| entry.child != slot.child);
            let (content, is_error) = match slot.outcome {
                ChildSlotOutcome::Generic => outcome(session, &slot.child, &status),
                ChildSlotOutcome::Vision {
                    images_inspected,
                    timed_out,
                } => crate::vision_child_outcome::outcome(
                    session,
                    &slot.child,
                    &status,
                    images_inspected,
                    timed_out,
                ),
            };
            ctx.emit(
                &slot.parent,
                RunnerEvent::ToolExecuted {
                    call_id: slot.call_id.clone(),
                    tool: Arc::from(slot.tool),
                    output_len: content.len(),
                    is_error,
                },
            );
            events.push(if is_error {
                Event::ToolFailed {
                    agent: slot.parent,
                    epoch: slot.epoch,
                    call_id: slot.call_id,
                    error: Arc::from(content),
                }
            } else {
                Event::ToolResult {
                    agent: slot.parent,
                    epoch: slot.epoch,
                    call_id: slot.call_id,
                    content: Arc::from(content),
                }
            });
        }
        events
    }

    /// 后台子（052）的收割：落终态的**进 stash，不回写父**，并从 detached 名单里
    /// 划掉。产出的事件数恒为零——父那个 spawn 槽在 spawn 那一刻就收敛了，这时候
    /// 再喂一条 `ToolResult` 进去就是一条对不上任何 `Pending` 槽的幽灵结果。
    ///
    /// # 红线 6 的回写校验点
    ///
    /// 判据跟 `Session::step` 入口那道闸**逐字一致**（`e != 当前世代` = 过期，
    /// 因为世代只增不减）：这份结果属于 spawn 那一刻的世界，中间要是被取消/undo
    /// 推过一代，它就该跟别的迟到回执一样被丢掉，**不进 stash**——stash 是给
    /// `collect` 领的，让一份已经被回滚掉的世界里的答案躺在那儿等人领，就是把
    /// 幽灵结果的落地点从消息历史挪到了另一张表。
    ///
    /// 这里必须自己比一次，不是重造机制：真正的门还是那一道（子自己的在飞
    /// provider 回执照样先过 `step` 的 epoch 闸），但 stash 这一步**不经过
    /// `Session::step`**（它不产出任何事件），没有别的地方替它把门。
    fn harvest_detached(&mut self, session: &Session) {
        let now = session.epoch();
        let mut i = 0;
        while i < self.detached.len() {
            // 绑了 collect 的（053）由上面那条槽位路负责回写，别再入一次 stash。
            if self.is_awaited(&self.detached[i].child) {
                i += 1;
                continue;
            }
            let status = session.status_of(&self.detached[i].child);
            if !status.is_terminal() {
                i += 1;
                continue;
            }
            let entry = self.detached.remove(i);
            if entry.epoch != now {
                continue;
            }
            let (content, is_error) = outcome(session, &entry.child, &status);
            self.stash.push(Stashed {
                child: entry.child,
                parent: entry.parent,
                content: Arc::from(content),
                is_error,
            });
        }
    }
}

#[cfg(test)]
#[path = "subtree_tests.rs"]
mod tests;
