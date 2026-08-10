//! 自动阶梯的接线点（108）：**turn 结束拿到 usage 时判一次，下一轮出料单时生效**
//! （096 第六问）。判读本身是 `agent_core::compaction::next_action`（纯函数，
//! 红线 1），这个文件只回答三件事——什么时候问它、拿什么当入参、答案怎么执行。
//!
//! # 为什么在「拿到 usage 之后」判，而不是「发请求之前」
//!
//! 096 第六问的原话：这样输入全是**已经落定的实测值**，同一份历史重放两次必然做出
//! 同一个压缩决定。挪到发请求前，入参里必然混进「这轮打算发什么」的估计值，
//! 重放当场不确定。
//!
//! 两个入参因此都取自已落定的事实：
//!
//! - `last_prompt_tokens` ← `Session::prev_prefix_of(root).prompt_tokens`。它是
//!   `provider_done` 那一格用**这次真实 usage** 回填进状态的（`prefix.prompt_tokens
//!   = Some(usage.prompt)`），所以它跟着 undo 一起回退，重放时逐字节一致。
//! - `context_window` ← 这个 agent 起飞时会用的那条 [`ExecutionBinding`] 的
//!   `SessionConfig.context_window`。`None`（未知/不设限）或者 binding 压根没配
//!   → 一档都不触发，**不许 `unwrap`**。
//!
//! # 「跨轮」在这里的形状：一轮只判一次
//!
//! 阶梯是**时间上的**（108 §「为什么阶梯是跨轮的」）：这一轮清工具结果，下一轮
//! 再实测；还超就说明清不动了，第 2 档自然返回空，第 3 档接手。
//!
//! 兑现它靠的不是什么额外机制，就是 [`Ladder`] 这个**一轮一次的闩**：一次
//! `resume` 里最多开火一次。没有这个闩的话，第 2 档清完之后泵下一次静止时再判一
//! 次，`tool_results_to_clear` 因为都进了 `plan.cleared()` 而返回空，第 3 档当场
//! 在同一轮里接上——「清完还不够」这句话就从**下一轮实测**退化成**同一轮的推断**，
//! 而那个推断需要 tokenizer 才做得准（红线 12）。108 验收第一条「造一个清工具返回
//! 够用的场景，第 3 档一次都没触发」正是冲着这个失效模式写的。
//!
//! # 只判 root
//!
//! `run_turn` 的契约是 root 中心的（返回的是 root 的终态），M12 这条线也是为
//! 「一条长会话的主干越来越长」做的。子 agent 不跨 turn（ORCHESTRATION §二），
//! 它们的历史活不到需要压缩的长度；摘要子 agent 自己更不该被压缩——那会递归。
//!
//! # 红线 12
//!
//! 这个文件里没有任何 provider 分支。`context_window` 是从 binding 上**读**下来的
//! 一个数，不是「这家是 DeepSeek 所以压得狠一点」——决策 17 已经把那条路堵死。

use agent_core::{
    AgentId, ClearParams, Effect, Event, LadderAction, Session, TurnStatus, compaction,
};

use crate::ctx::RunnerCtx;
use crate::event::RunnerEvent;
use crate::persist;

/// 一轮的阶梯闩：判读时机 + 「只判一次」。`resume` 每次重建，跟
/// [`crate::compact_slot::CompactSlots`] / [`crate::subtree::Subtree`] 同款
/// turn 内生死。
pub(crate) struct Ladder {
    root: AgentId,
    /// 这一轮 root 有没有真的拿到一份实测用量（一条属于它的 `ProviderDone`）。
    ///
    /// 光看「root 到终态了没」不够：`Done` 还有两条**没有新观测**的到达路径——
    /// 终态上又收到一条 `UserInput`（调用方忘了 `begin_turn`，协议违规，状态不变
    /// 但仍是终态），以及轮预算在开轮那一刻就已经用尽（`Idle + UserInput` 直接落
    /// `Done{truncated:true}`，一次请求都没发）。这两条路上 `prev_prefix` 装的是
    /// **上一轮**的实测值，而那一份已经被上一轮的阶梯判过了；拿它再判一次，
    /// 第 2 档会因为「都清过了」返回空，于是凭空多开一次第 3 档——烧一次模型调用，
    /// 而「清完还不够」从来没有被实测过。
    measured: bool,
    fired: bool,
}

impl Ladder {
    pub(crate) fn new(root: AgentId) -> Self {
        Ladder {
            root,
            measured: false,
            fired: false,
        }
    }

    /// 在 `session.step(event)` **之前**过一眼这条事件：它是不是 root 的一次实测。
    pub(crate) fn note(&mut self, event: &Event) {
        if let Event::ProviderDone { agent, .. } = event
            && agent == &self.root
        {
            self.measured = true;
        }
    }

    /// turn 结束了就判一次（整轮只判一次），返回这一档要执行的 effect。
    ///
    /// 第 2 档不产出 effect——清工具结果是一条**命令**（101），当场写完就完了，
    /// 下一轮出料单时 `project` 自然拿到新的 `SendPlan`（099），而 103 的意图判读
    /// 会因为 `SendPlan` 跟上一次发出去的那份不等而落 `PrefixIntent::Intentional`，
    /// 兜底第 1 层不会把这次预期内的漂移报成事故（096 §注意）。
    ///
    /// 第 3 档产出一条 `Effect::Compact`——摘要要烧一次模型调用，是真正的在飞异步，
    /// 走 105 定的那条 effect/事件接缝（`dispatch` → `compact_spawn::intercept`）。
    /// 调用方把它并进这一步的 effect 批里，跟 core 自己产出的 effect 同一条路派发。
    pub(crate) fn fire_once(&mut self, session: &mut Session, ctx: &mut RunnerCtx) -> Vec<Effect> {
        if self.fired || !self.measured {
            return Vec::new();
        }
        // 只在 `Done` 上判。`Failed(Cancelled)` / `Failed(Provider(_))` 收尾的一轮
        // 不压：用户刚按下取消，紧接着起一个摘要子 agent 是最不该发生的事；而失败
        // 那一轮的实测值属于**上一次**成功的请求，跟上面 `measured` 是同一条理由。
        if !matches!(session.status_of(&self.root), TurnStatus::Done { .. }) {
            return Vec::new();
        }
        self.fired = true;

        match decide(session, ctx, &self.root) {
            LadderAction::Nothing => Vec::new(),
            LadderAction::ClearToolResults(ids) => {
                let outcome = session.clear_tool_results(&self.root, ids);
                if !outcome.newly_cleared.is_empty() {
                    persist::sync(ctx, session);
                    // 109：被清的调用要能在时间线上标出来，不是凭空消失。只在
                    // 真有新东西被清时才发（`already_cleared`/`unknown` 幂等
                    // 命中不该在时间线上重复冒出同一条标记）。
                    ctx.emit(
                        &self.root,
                        RunnerEvent::ToolResultsCleared {
                            turn_id: session.turn_id(),
                            call_ids: outcome.newly_cleared.clone(),
                        },
                    );
                }
                Vec::new()
            }
            LadderAction::Summarize { upto } => vec![Effect::Compact {
                agent: self.root.clone(),
                upto,
                epoch: session.epoch(),
            }],
        }
    }
}

/// 组入参、问纯函数。**没有任何判断在这里**——判断全在
/// `compaction::next_action` 里，这样「同一份历史重放两次决定相同」是它一个函数
/// 的性质，不是散在宿主侧的一堆条件的合取。
fn decide(session: &Session, ctx: &RunnerCtx, root: &AgentId) -> LadderAction {
    let profile = session.execution_profile_of(root);
    let context_window = ctx
        .execution_binding_for(profile.as_ref())
        .ok()
        .and_then(|selection| selection.binding.session_config.context_window);
    let last_prompt_tokens = session
        .prev_prefix_of(root)
        .and_then(|prefix| prefix.prompt_tokens);

    compaction::next_action(
        &session.messages_of(root),
        &session.send_plan_of(root),
        last_prompt_tokens,
        context_window,
        ClearParams::default(),
    )
}

#[cfg(test)]
#[path = "compact_ladder_tests.rs"]
mod tests;
