//! `SessionActor`：一个 session 的全部状态独占一个 `std::thread`（issue 030，
//! ARCHITECTURE.md 关键判断 1）。这个模块只做一件事——起线程、握手、崩溃隔离；
//! 线程内部真正做的事（现造 `Session`/`RunnerCtx`、进命令循环）在 [`body`]，
//! 每条命令怎么落到 `Session` 上在 [`commands`]。
//!
//! # 为什么 `Session`/`RunnerCtx` 必须现造在线程内部，不能从外面 `move` 进来
//!
//! 两者都含 `Rc<RefCell<_>>`（`agent-store` 的 `Store`），`!Send`——想在调用
//! [`spawn`] 的线程上先构造好再塞进 `thread::spawn` 的 `move` 闭包，编译期就
//! 会被 `Send` bound 拦下来。所以 [`crate::registry::OpenSpec`] 只装
//! `Send + 'static` 的**配置**，`Session::new`/`agent_runtime::recover`/
//! `RunnerCtx::new` 全部挪进 [`body::run`]，在新线程里现场建。
//!
//! # 崩溃隔离：`catch_unwind` 包住整条命令循环
//!
//! 线程闭包本身只做两件事：`catch_unwind` 跑 [`body::run`]；`Err`（真的
//! panic 了）就把 panic 负载翻成一句话，写进 `died` 单元格、广播
//! [`SessionEvent::SessionDied`]。`std::thread` 本来就是 panic 传播的边界
//! （子线程 panic 不会拖垮进程），这里只是把「外界怎么知道」接上：
//! `SessionRegistry::get`/`close` 读 `died` 就能报出死因，事件流的订阅者立刻
//! 收到终态——不是进程崩、也不是静默消失。
mod body;
mod capabilities;
mod commands;

use std::panic::AssertUnwindSafe;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use tokio::sync::broadcast;

use agent_core::{AgentId, AgentTree};
use agent_runtime::RemoteToolWaiting;

use crate::event::{Frame, SessionEvent};
use crate::handle::{CancelHandle, SessionHandle};
use crate::registry::OpenSpec;

use body::ReadyMsg;

/// `broadcast` 的环形缓冲容量（issue 030 原文点名 256）。慢订阅者跟不上就会
/// `Lagged`——`crate::handle::Subscription::recv` 把它翻成显式的
/// [`SessionEvent::Lagged`]，不是本模块的事，这里只定容量。
pub const EVENT_CHANNEL_CAPACITY: usize = 256;

/// actor 启动阶段的失败：恢复 `Session`、建 `ToolExecutor` 之类的构造性错误，
/// 在真正进入命令循环**之前**就出错了。跟运行时的 panic（`died` 单元格里的
/// 死因）是两回事——这个错误发生时 `SessionHandle` 从未诞生，调用方拿到的
/// 是 `Err`，不是一个已经死掉的 handle。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenError(pub String);

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for OpenError {}

/// [`spawn`] 成功之后交给 [`crate::registry::SessionRegistry`] 的三样东西：
/// 外界用的 [`SessionHandle`]、优雅关闭时要 `join` 的线程句柄、判断「活着还是
/// 死了」要读的死因单元格。开一个具名结构体而不是裸元组——三个字段各管各的
/// 一件事，元组下标没有这种自解释性（也顺手躲开 clippy 的 type_complexity）。
pub(crate) struct SpawnedActor {
    pub(crate) handle: SessionHandle,
    pub(crate) join: thread::JoinHandle<()>,
    pub(crate) died: Arc<Mutex<Option<String>>>,
}

/// 起一个 session actor。
pub(crate) fn spawn(spec: OpenSpec) -> Result<SpawnedActor, OpenError> {
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (events_tx, _initial_receiver) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
    let (ready_tx, ready_rx) = mpsc::channel::<ReadyMsg>();
    let died: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    // 048：GET `/sessions/:id/agents` 读的共享单元格。造在这里（不是握手之后
    // 才由线程内部传回来）是因为 `AgentTree` 是普通 `Send` 数据（不像
    // `Session`/`RunnerCtx` 那样含 `Rc<RefCell<_>>`），不需要走 `ReadyMsg`
    // 那趟握手才能拿到——直接建一个空快照占位（`body::run` 会在真正的
    // `Session` 现造出来的第一时间用 `agent_tree()` 覆盖它，`open()` 返回
    // 之前这个占位永远不会被外界看到）。
    let tree: Arc<Mutex<AgentTree>> = Arc::new(Mutex::new(AgentTree { nodes: Vec::new() }));
    // 072：`GET /sessions/:id/pending_tools` 读的共享单元格，同上一条逐行同款。
    // 空 `Vec` 是**真实**初值（不是占位）：一个刚起来的会话确实一件远端活都没欠，
    // 所以这里不像 `tree` 那样需要 `body::run` 在握手前覆盖一次。
    let pending_tools: Arc<Mutex<Vec<RemoteToolWaiting>>> = Arc::new(Mutex::new(Vec::new()));

    let thread_name = format!("session-actor-{}", spec.id);
    let events_for_thread = events_tx.clone();
    let died_for_thread = Arc::clone(&died);
    let tree_for_thread = Arc::clone(&tree);
    let pending_for_thread = Arc::clone(&pending_tools);

    let join = thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            // 这两份克隆专供 panic 分支用：`body::run` 会拿走 `events_for_thread`/
            // `ready_tx` 的所有权，panic 之后原件已经跟着栈一起被展开丢弃了，
            // 广播终态、补发握手失败信号得靠这里预先留的另一份。
            let events_for_panic = events_for_thread.clone();
            let ready_for_panic = ready_tx.clone();
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                body::run(spec, cmd_rx, events_for_thread, ready_tx, tree_for_thread, pending_for_thread);
            }));
            if let Err(payload) = result {
                // `&*payload`，不是 `&payload`：`payload: Box<dyn Any + Send>`，
                // 而 `Box<dyn Any + Send>` 自己也满足 `Any`（一切 `'static`
                // 类型都满足）——`&payload` 会把整个 `Box` 强转成 `&dyn Any`，
                // `downcast_ref::<&str>()` 就永远拿不到真正的负载类型，只会
                // 落进「不是字符串」那个分支（这个仓库独测踩过一次的真事故，
                // 不是防御性写法）。先解引用一层，`&*payload` 才是负载本身。
                let reason = panic_message(&*payload);
                *died_for_thread.lock().unwrap() = Some(reason.clone());
                // 事件流收到的最后一条：线程即将退出。034：标 root——这是
                // actor/连接级的事实，不属于树上任何一个具体 agent。
                let _ = events_for_panic.send(Frame {
                    agent: AgentId::root(),
                    event: SessionEvent::SessionDied { reason: reason.clone() },
                });
                // 万一 panic 发生在 `body::run` 送出握手信号之前（比如
                // `ToolExecutor::new` 内部真的 panic 了，而不是走它自己的
                // `Result` 出口），`spawn` 还在等这条握手——补一条失败信号，
                // 不让它永远挂着。`body::run` 已经握过手的情况下，这次多发的
                // `Err` 只是被 `spawn` 忽略（它只读第一条），无害。
                let _ = ready_for_panic.send(Err(reason));
            }
        })
        .expect("起 session actor 线程失败（系统资源耗尽一类），这个仓库其它地方对 std::thread::spawn 失败的既有处理方式也是让它 panic，不是这个 issue 要新定义的错误分类");

    match ready_rx.recv() {
        Ok(Ok(cancel)) => {
            let handle = SessionHandle { canceller: CancelHandle::new(cmd_tx, cancel), events: events_tx, tree, pending_tools };
            Ok(SpawnedActor { handle, join, died })
        }
        Ok(Err(reason)) => {
            let _ = join.join();
            Err(OpenError(reason))
        }
        Err(_) => {
            let _ = join.join();
            Err(OpenError("actor 线程异常退出，未能确认启动状态".to_string()))
        }
    }
}

/// 把 panic 负载翻成人能读的一句话。`std::panic!` 的常见负载是 `&'static str`
/// 或者 `String`（`format!` 触发的那种），两个都接住；接不住的类型（自定义
/// panic payload）退回一句诚实的「不是字符串」，不猜内容。
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "actor 线程 panic，负载不是字符串".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 回归测试：`catch_unwind` 的 `Err` 负载是 `Box<dyn Any + Send>`——
    /// 而 `Box<dyn Any + Send>` 自己也满足 `Any`（一切 `'static` 类型都满足），
    /// 所以调用方一旦手滑传 `&payload`（`&Box<..>`）而不是 `&*payload`，
    /// 就会把整个 `Box` 强转成 `&dyn Any`，`downcast_ref::<&str>()`/
    /// `downcast_ref::<String>()` 永远落空——这不是假设，是本文件独测时
    /// 真的踩过的事故（`spawn` 里 `panic_message(&payload)` 一度就是错的那个
    /// 版本，`actor_panic_is_reported_dead.rs` 断言死因文本时才炸出来）。
    /// 这里直接钉住 `panic_message` 本身，不必每次都绕一整圈真的起线程。
    #[test]
    fn panic_message_recovers_str_and_string_payloads_from_a_boxed_any() {
        let str_payload: Result<(), Box<dyn std::any::Any + Send>> =
            std::panic::catch_unwind(AssertUnwindSafe(|| panic!("literal payload")));
        let reason = panic_message(&*str_payload.unwrap_err());
        assert_eq!(reason, "literal payload");

        let string_payload: Result<(), Box<dyn std::any::Any + Send>> =
            std::panic::catch_unwind(AssertUnwindSafe(|| panic!("formatted {}", "payload")));
        let reason = panic_message(&*string_payload.unwrap_err());
        assert_eq!(reason, "formatted payload");
    }
}
