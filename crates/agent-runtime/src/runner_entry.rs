//! 一轮从哪开始：泵的**三个入口**，以及它们各自允许重置什么。
//!
//! 泵本体在隔壁 [`runner`](super::runner)。拆开（214，红线 9：`runner.rs` 顶破
//! 500 行硬上限）的判据不是行数——这三个函数**只在「进泵之前做了什么」上不同**，
//! 而那恰恰是最容易写错、且写错不报错的一格：
//!
//! | 入口 | 清取消标志 | 排空 `NextTurn` 收件箱 | 谁调 |
//! |---|---|---|---|
//! | [`run_turn_async`] | ✅ 每轮一次 | ✅ **这就是 `Deliver::NextTurn` 的定点** | 宿主，新的一轮用户输入 |
//! | [`run_turn`] | 同上（它就是同步壳） | 同上 | native 宿主 |
//! | [`resume_async`] | ❌ | ❌ | 远端工具回传续跑 |
//!
//! `resume_async` 那两个 ❌ 是本文件存在的全部理由：远端工具的回传**不是新的用户
//! 轮次**。清了取消标志，用户在等待期间按下的 Ctrl-C 就被抹掉了；排空了
//! `NextTurn`，一条本该等到下一轮的留言会在半轮中间冒出来。两样都不报错。
//!
//! `begin_turn` 一个入口都不调——turn 边界是**调用方**的事（026 判断 13：不藏进
//! 转移表，也不藏进泵）。

use std::sync::Arc;
use std::sync::atomic::Ordering;

use agent_core::{Event, Session, TurnStatus};

use crate::ctx::RunnerCtx;
use crate::runner::resume_after_first_commit;
use crate::transient_source_failure::TransientSourceFailure;

/// 跑一整轮：喂 `user_input`，驱动整棵 agent 树直到没有任何东西在飞。
///
/// `session` 原地推进，返回值只是 **root** 终态的一份拷贝，方便调用方立即判断
/// 结果——真正的历史/状态变化都已经写进 `session`（并已经同步进持久化后端）。
///
/// 正常收尾的状态仍在 `Ok` 中；如果一条消耗 transient source 的 provider 调用
/// 无法收尾，原始失败事实在 `Err` 中交给嵌入宿主。宿主决定错误策略，泵只负责
/// 释放本轮的 transient source 状态。
///
/// 取消标志只在这里清零一次（每轮开始各清一次，理由跟 022 的 `agent-cli::
/// repl::run` 一致：上一轮遗留的标志不该提前打断这一轮还没开始的请求）；
/// 轮内的重试**不**清——那会把重试等待期间真实按下的 Ctrl-C 抹掉。
///
/// **调用方必须先 `session.begin_turn()`**（除了会话的第一轮，`Session::new`
/// 已经是 `Idle`）——`Session::begin_turn` 是显式命令（026 判断 13：turn 边界
/// 是会话层面的概念，不藏进转移表），这个函数不替调用方决定「新一轮从哪开始」。
///
/// 这是泵本体，两个目标共用；native 上另有一个同名不带后缀的同步壳
/// （[`run_turn`]），见模块文档「但公开入口在 native 上仍然是同步的」。
pub async fn run_turn_async(
    session: &mut Session,
    ctx: &mut RunnerCtx,
    user_input: &str,
) -> Result<TurnStatus, TransientSourceFailure> {
    ctx.cancel.store(false, Ordering::Relaxed);
    let root = session.agent().clone();
    // 206：**这就是 `Deliver::NextTurn` 的定点**——上一轮留给 root 的话，在这一轮
    // 的第一条 user 消息**之前**进历史。
    //
    // 顺序是刻意的：那条留言是上一轮末尾说的，时间上排在用户这句新话前面。
    //
    // 位置也是刻意的：调用方已经调过 `session.begin_turn()`（见本函数文档），
    // 所以这条 entry 属于**新**的 turn——`/undo` 掉新这一轮会把留言退回收件箱，
    // 老那一轮不受影响。放在 `begin_turn` 之前，它会挂在上一轮尾巴上，
    // undo 掉上一轮就把一条还没被读过的话一起吞了。
    //
    // 只在 `run_turn_async` 里做，**不在 `resume_async`**：远端工具的回传不是
    // 新的用户轮次（见 `resume_async` 的文档），那条路上没有新的 turn 边界。
    session.drain_next_turn();
    resume_async(
        session,
        ctx,
        Event::UserInput {
            agent: root,
            text: Arc::from(user_input),
        },
    )
    .await
}

/// [`run_turn_async`] 的同步壳：把整条 await 链在调用线程上跑到底。
///
/// **签名与 116 之前逐字一致**，所以 `agent-cli`、`agent-server` 的 actor 线程
/// 和所有集成测试一个字都不用改。行为也逐字一致：它们本来就是「在一条裸
/// `std::thread` 上把这一轮跑完」，只是「等」的实现从 `recv_timeout` 换成了
/// [`crate::block_on`] 的 `thread::park`。
///
/// **wasm 上没有这个壳**（`cfg` 掉了），不是遗漏：`block_on` 靠停住当前线程来
/// 等，浏览器主线程一停，驱动 `fetch` 的事件循环跟着停 = 死锁。wasm 宿主直接用
/// [`run_turn_async`]，由浏览器的事件循环驱动。
///
/// 顺带说清这处 `cfg` 为什么可接受：它长在**公开 API 的便利壳**上，不在核心执行
/// 逻辑里——泵本体、IO 载体、落地规则两个目标共用同一份代码（红线 12 管的是
/// core 里的平台判断）。
#[cfg(not(target_arch = "wasm32"))]
pub fn run_turn(
    session: &mut Session,
    ctx: &mut RunnerCtx,
    user_input: &str,
) -> Result<TurnStatus, TransientSourceFailure> {
    crate::block_on(run_turn_async(session, ctx, user_input))
}

/// 从一项已发生的事件继续驱动会话。
///
/// 远端工具的回传不是新的用户轮次，不能清除取消标志或调用 `begin_turn`；因此由
/// 受控的远端回传入口走这里，和普通工具执行完成后进入泵的路径完全一致。
pub(crate) async fn resume_async(
    session: &mut Session,
    ctx: &mut RunnerCtx,
    initial: Event,
) -> Result<TurnStatus, TransientSourceFailure> {
    resume_after_first_commit(session, ctx, initial, |_| {}).await
}
