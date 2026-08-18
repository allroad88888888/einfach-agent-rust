//! 206：轮末盘点**还有多少话没被读到**。
//!
//! 这个文件只做一件事：root 落终态时，扫一遍活着的 agent，把收件箱里还剩的
//! `Deliver::Now` 条目报成 [`RunnerEvent::UnreadMessages`]。
//!
//! # 为什么会剩下
//!
//! `Deliver::Now` 的排空定点在收信人**下一次组装 provider 请求之前**
//! （`dispatch` 的 `CallProvider` 臂）。所以一条话没被读到，只有一个原因：
//! **收信人在这一轮里再也没有说过话**——多半是发的时候它已经答完了。
//!
//! 206 明确**不唤醒**它（`Effect::CallProvider` 全系统只从 `try_call_provider`
//! 一处发出，四个入口都要求那个 agent 正走在流程里；唤醒要新增 core 转移，
//! 是 issue 214）。所以这里能做的就是**说一声**，而不是补救。
//!
//! # `NextTurn` 的条目不算
//!
//! 它们**本来就该留到下一轮**——那是这一档存在的全部意义。206 §4 把这条单列成
//! 「第二容易写错的地方」：孤儿收尾今天是「收件箱非空就告警」的直觉写法，
//! 加了 `NextTurn` 之后那个直觉会**把正常情况报成异常**，接着有人会「顺手清干净」。
//!
//! # 不改轮次结果，也不让泵多转一圈
//!
//! 跟 [`crate::orphan`] 同一条规矩（ORCHESTRATION §四.4）：一轮结束就是结束。
//! 这里只 `ctx.emit` 一条通报，一个字节的会话状态都不写——**连 command 都没有**，
//! 所以红线 2 在这个文件里是「根本没有写入路径」而不是「小心走正门」。

use agent_core::{AgentId, Deliver, Session};

use crate::ctx::RunnerCtx;
use crate::event::RunnerEvent;

/// 扫一遍活着的 agent，报出还没被读到的 `Now` 条目。
///
/// **调用点在 `runner` 的 B0，`orphan::reap` 之前**：reap 会把没人领的后台子
/// `despawn_child` 掉，它们的收件箱 atom 跟着被逐出——报在后面就一条都看不到了，
/// 而「给一个已经答完的子 agent 发了话」恰恰是这条告警最想抓的场景。
///
/// 调用方负责**只报一次**（泵在终态上可能转不止一圈）。
pub(crate) fn report(session: &Session, ctx: &mut RunnerCtx) {
    for agent in live_sorted(session) {
        let count = session
            .inbox_of(&agent)
            .iter()
            .filter(|item| item.when == Deliver::Now)
            .count();
        if count > 0 {
            ctx.emit(
                &agent,
                RunnerEvent::UnreadMessages {
                    agent: agent.clone(),
                    count,
                },
            );
        }
    }
}

/// 活着的 agent，按 `AgentId` 排序。
///
/// **自己排一次**，不借 `live_agents()` 的排序承诺：告警的顺序会进 CLI 面板和
/// SSE 帧，被调方哪天改了排序，坏的是这里的输出而不是它自己的测试。
/// 跟 `status_tool::all_agents` 是同一条理由。
fn live_sorted(session: &Session) -> Vec<AgentId> {
    let mut live = session.live_agents();
    live.sort();
    live
}
