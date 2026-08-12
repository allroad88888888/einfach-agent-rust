//! 一次宿主工具执行的 `await` 怎么被**打断**（123）：两条互相独立的路。
//!
//! 121 之前 `host_tool::execute` 是瞬时的，「执行期间」这个时间段结构上不存在。
//! 可等待之后它成了这个 crate 里**唯一一段长度由页面说了算**的时间：页面的
//! Promise 想挂多久挂多久，也可以永不 settle。而 `AgentHost::send` 在整轮期间
//! 握着 `live.borrow_mut()`（[`crate::host_session`] 的借用纪律），所以这段时间挂住
//! ≠ 一次调用变慢，而是**整个宿主对页面失去响应**：`sessionId()`/`historyJson()`
//! 借不到、`deleteSession()` reject、`send()` 那个 Promise 永远不 settle。
//!
//! 于是这里给那次 `await` 加两条出口。**它们是两件事，不是一件事的两种说法**：
//!
//! | | 谁触发 | 判据 | 到了怎么收 |
//! |---|---|---|---|
//! | 取消 | **用户**（`AgentHost::cancel()`） | `RunnerCtx` 的取消标志（`AtomicBool`，`run_turn_async` 每轮清一次） | `cancel_pending_remote_tools_async` → `Event::Cancel` → 取消轮丢弃 |
//! | 截止线 | **时间** | 等待槽登记那一刻算好的绝对时刻（060 的账，`agent_runtime::remote_tool_deadline_in` 问它） | `sweep_remote_tool_deadlines_async` → 一条 `is_error` 的工具结果喂回模型 |
//!
//! 两者都能单独触发：没超时也能取消，没人取消也会到点。收尾动作在 [`crate::turn`]
//! ——这个文件只负责「等到其中一件事发生」。
//!
//! # 取消为什么能立刻醒，而不是靠轮询
//!
//! 页面调 `cancel()` 只是翻一个 `AtomicBool`。单线程的浏览器里没有第二个线程会
//! 替我们发现它，而我们正停在页面 Promise 那个 await 点上——**不叫醒就得等到
//! Promise 自己 settle**，取消也就成了「等挂住的东西结束再生效」，等于没有。
//!
//! 所以 [`until_settled`] 在每次 poll 时把自己的 `Waker` 存进一个线程局部槽，
//! `cancel()` 翻完标志顺手 [`wake`] 一下。手法与存放位置跟
//! [`crate::callback`] 那个 `ACTIVE_TOOL_SLOT` 同款、理由也同款（wasm 主线程，
//! 这个 crate 本来就 `Rc`/`RefCell` 满地）。备选是每 100ms 醒一次自己看标志，
//! 被否掉的理由不是「慢一点」而是**后台标签页里 `setTimeout` 会被节流到 1s 以上**
//! ——取消延迟会随着标签页是否可见而变，那是最难复现的一类问题。
//!
//! # JS 的 Promise 没法真 abort：这里选的是 (b)
//!
//! 打断只是**我们不再等它**：那次 `JsFuture` 被丢掉，页面回调里已经飞出去的
//! `fetch` 照样跑完（浪费一次调用），只是它 resolve 时 Rust 这边连接收的东西都
//! 没有了。备选 (a)——给回调多传一个 `AbortSignal` 让页面自己响应——**没做**：
//! 页面不配合就等于没有，而 API 面立刻变大一档；(b) 不依赖页面配合，会话立刻干净。
//! 两者不冲突，(a) 将来要加就是在 (b) 之上多传一个参数。
//!
//! 「结果不会写进状态」这件事因此有两道闸，而且**都不靠页面守规矩**：丢掉的
//! `JsFuture` 让晚到的结果压根回不到 Rust；万一从别的入口回来了，等待槽也早已被
//! 取消/到点划掉（`take_remote_tool` 找不到 → `ResolveRemoteToolError::InvalidResult`，
//! [`crate::turn`] 那条既有分支原样处理）。

use std::cell::RefCell;
use std::future::Future;
use std::sync::atomic::Ordering;
use std::task::{Poll, Waker};
use std::time::Duration;

use agent_runtime::{RemoteToolWaiting, RunnerCtx};
use wasm_bindgen_futures::JsFuture;

/// 这次执行是被什么打断的。**没有第三个变体**：正常返回走 `Ok`。
pub(crate) enum Interrupted {
    /// 用户按了取消（`AgentHost::cancel()` 翻的那个标志）。
    Cancelled,
    /// 这条等待槽的截止线到了。
    Expired,
}

thread_local! {
    /// 正停在工具 `await` 上的那个任务的 `Waker`。见模块文档第二节。
    static PARKED: RefCell<Option<Waker>> = const { RefCell::new(None) };
}

/// 叫醒正停在工具 `await` 上的那一轮，让它立刻看一眼取消标志。
///
/// 由 `AgentHost::cancel()` 调，**不自己翻标志**——标志是谁的、什么时候翻，是
/// `cancel()` 的事；这里只负责「让在等的人醒过来看一眼」。没人在等就是空操作。
pub(crate) fn wake() {
    let waker = PARKED.with(|parked| parked.borrow_mut().take());
    if let Some(waker) = waker {
        waker.wake();
    }
}

/// 等 `fut` 出结果，**或者**等到用户取消 / 这条等待槽到点。
///
/// `ctx` 只读两样东西：取消标志和 `waiting` 这一条槽的剩余时间。截止线用的是
/// `agent_runtime` 那份（登记等待槽时按 `RunnerCtx::with_remote_tool_timeout` 的预算
/// 算好的绝对时刻），**这里不另立一个数**——两份预算哪天不一致，症状是「到点了却
/// 扫不出过期槽」，那会变成一个原地空转的循环。
///
/// 拿不到 `window`（不是页面主线程）或者 `setTimeout` 装不上时**退化成只剩取消
/// 这一条出口**：不装假的截止线，也不因此拒绝执行。
pub(crate) async fn until_settled<F: Future>(
    ctx: &RunnerCtx,
    waiting: &RemoteToolWaiting,
    fut: F,
) -> Result<F::Output, Interrupted> {
    let cancel = ctx.cancel_flag();
    let mut fut = std::pin::pin!(fut);
    let outcome = loop {
        // 进来先看一眼：`cancel()` 可能发生在上一个 await 点到这里之间（那时还没有
        // 谁的 `Waker` 存在槽里，[`wake`] 是空操作），漏看就要多等一整个工具。
        if cancel.load(Ordering::Relaxed) {
            break Err(Interrupted::Cancelled);
        }
        let remaining =
            agent_runtime::remote_tool_deadline_in(ctx, &waiting.agent, &waiting.call_id);
        if remaining.is_some_and(|remaining| remaining.is_zero()) {
            break Err(Interrupted::Expired);
        }
        let mut timer = remaining.and_then(sleep).map(Box::pin);
        let step = std::future::poll_fn(|cx| {
            // 取消排在最前面：取消之后一律不再收结果，哪怕这一刻工具其实已经好了
            // ——「取消之后状态一个字节不变」这句话因此不需要任何附加条件。
            if cancel.load(Ordering::Relaxed) {
                return Poll::Ready(Step::Cancelled);
            }
            if let Poll::Ready(output) = fut.as_mut().poll(cx) {
                return Poll::Ready(Step::Settled(output));
            }
            if let Some(timer) = timer.as_mut()
                && timer.as_mut().poll(cx).is_ready()
            {
                return Poll::Ready(Step::Elapsed);
            }
            park(cx.waker());
            Poll::Pending
        })
        .await;
        match step {
            Step::Settled(output) => break Ok(output),
            Step::Cancelled => break Err(Interrupted::Cancelled),
            // 定时器响了**不直接判超时**：回循环开头拿绝对时刻复核一次。浏览器的
            // 定时器只保证「不早于」，但四舍五入到毫秒之后差一丁点是可能的，而
            // `sweep_remote_tool_deadlines_async` 判过期用的是绝对时刻——它俩要是
            // 各判各的，就会出现「这边判超时、那边扫不出过期槽」的空转。
            Step::Elapsed => continue,
        }
    };
    // 醒来之后这个槽就没意义了；留着不会错（叫醒一个已经结束的任务是安全的），
    // 但留着就得让下一个读者去想「会不会叫错人」。
    unpark();
    outcome
}

/// 一次 poll 的三种去向。`Settled` 带着 `fut` 的产出，所以它是泛型的。
enum Step<T> {
    Settled(T),
    Cancelled,
    Elapsed,
}

fn park(waker: &Waker) {
    PARKED.with(|parked| {
        let mut parked = parked.borrow_mut();
        let already = parked.as_ref().is_some_and(|old| old.will_wake(waker));
        if !already {
            *parked = Some(waker.clone());
        }
    });
}

fn unpark() {
    PARKED.with(|parked| *parked.borrow_mut() = None);
}

/// `setTimeout` 包成一个 future。`None` = 这个宿主装不上定时器（不是页面主线程，
/// 或者 `setTimeout` 本身报错），调用方据此退化成「没有截止线」。
///
/// 不配 `clearTimeout`：被丢掉的定时器到点还是会响一次，响的时候那个 Promise 已经
/// 没人接了，代价是一次空回调。整轮里每条工具至多装一个，不值得为它多存一个 id。
fn sleep(duration: Duration) -> Option<JsFuture> {
    let window = web_sys::window()?;
    let millis = i32::try_from(duration.as_millis()).unwrap_or(i32::MAX);
    let mut armed = false;
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        armed = window
            .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, millis)
            .is_ok();
    });
    armed.then(|| JsFuture::from(promise))
}
