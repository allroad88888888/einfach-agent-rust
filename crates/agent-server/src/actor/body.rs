//! actor 线程真正跑的东西：现造 `Session` + `RunnerCtx`（两者 `!Send`，只能在
//! 这个线程内部诞生，见 `super` 模块文档），握手告诉 `open()` 「起好了还是
//! 起失败了」，然后进入命令循环直到 `Shutdown` 或者队列的发送端被丢弃。

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;

use tokio::sync::broadcast;

use agent_core::{AgentId, Session, SessionConfig};
use agent_runtime::{AgentEvent, RunnerCtx};
use agent_tools::ToolExecutor;

use crate::command::Command;
use crate::event::{Frame, SessionEvent};
use crate::registry::OpenSpec;

use super::commands;

/// 握手消息：`Ok(cancel)` = 起好了，`cancel` 是 [`RunnerCtx::cancel_flag`]
/// （[`crate::handle::SessionHandle::cancel`] 直接旁路写的那个原子标志）；
/// `Err(reason)` = 恢复/构造阶段就失败了，线程即将退出、从未进入命令循环。
pub(super) type ReadyMsg = Result<Arc<AtomicBool>, String>;

/// 广播一条不属于任何具体 agent 的事件——actor/连接级的事实（落盘 IO 麻烦、
/// 快照里认不出的键之类），标 [`AgentId::root`]（034：`crate::event::frame`
/// 模块文档同一条判据）。
fn emit_root(events_tx: &broadcast::Sender<Frame>, event: SessionEvent) {
    let _ = events_tx.send(Frame { agent: AgentId::root(), event });
}

pub(super) fn run(spec: OpenSpec, rx: mpsc::Receiver<Command>, events_tx: broadcast::Sender<Frame>, ready_tx: mpsc::Sender<ReadyMsg>) {
    let agent = AgentId::root();
    let history_cap = spec.history_cap.unwrap_or(agent_core::DEFAULT_HISTORY_CAP);

    // 落盘 IO 层面的麻烦（打不开文件、写失败）借用「传输/IO 麻烦」这一类事件
    // 广播出去——`SessionEvent` 没有为它单独开变体：对客户端来说，「底层通道
    // 出了岔子，文本里有细节」是同一件事，不管这条通道是 HTTP 还是本地文件。
    let events_for_store_errors = events_tx.clone();
    let store = agent_runtime::open_backend(spec.store_path.clone(), move |e| {
        emit_root(&events_for_store_errors, SessionEvent::TransportTrouble(Arc::from(e.to_string())));
    });

    let mut unknown_keys: Vec<String> = Vec::new();
    let recovered = agent_runtime::recover(store.as_ref(), agent.clone(), history_cap, &mut |key| {
        unknown_keys.push(format!("{key:?}"));
    });

    let mut session = match recovered {
        Ok(Some(session)) => session,
        Ok(None) => {
            let mut session = Session::new(agent.clone());
            if spec.history_cap.is_some() {
                session.set_history_cap(Some(history_cap));
            }
            // 034：`ToolTableSpec::Full` 带的 spawn 上限对齐进 `Session`——
            // `ToolTable::with_spawn` 只把这组数字写进给模型看的描述，真正拦
            // 人的两道闸在 `Session::spawn_child`，两边必须是同一组数
            // （`ToolTableSpec::spawn_limits` 文档）。只在新建会话时对齐，跟
            // `history_cap` 同一个既有取舍：恢复出来的会话带着它自己持久化过
            // 的配置，不被这一刻的服务端配置悄悄改写。
            if let Some(limits) = spec.tools.spawn_limits() {
                session.set_agent_limits(limits);
            }
            session
        }
        Err(e) => {
            // 恢复失败是硬失败，不吞不猜（`agent_runtime::persist::recover` 模块
            // 文档的「诚实原则」，`agent-cli` 的 main.rs 对同一个错误的处理是
            // 直接拒绝启动进程——这里对应的是「拒绝启动这个 actor」）。
            let _ = ready_tx.send(Err(e.to_string()));
            return;
        }
    };

    // 快照里认不出的键不是硬失败（`recover` 忽略了它们，继续往下走），但也
    // 不能悄悄吞掉——`agent-cli` 的 main.rs 对同一个回调是 `eprintln!`，这里
    // 没有 stderr 可打，借用同一个「底层通道有话要说」的事件桶广播出去（跟
    // 落盘 IO 错误共用 `TransportTrouble`，见上面 `open_backend` 的注释）。
    // **老实说这条大概率没人听得到**：这一刻 `open()` 还没返回，调用方连
    // `SessionHandle` 都拿不到，自然订阅不上——跟上面 `open_backend` 的
    // `on_error` 回调在 `store.load()` 阶段触发时是同一个结构性限制。发出去
    // 仍然比不发好：无害（没有订阅者时 `send` 只是返回一个被忽略的 `Err`），
    // 且一旦 031/未来的事件环形缓冲把「补发」接上，这条历史信息不会因为
    // 这里选择不发而永久丢失。
    for key in &unknown_keys {
        emit_root(
            &events_tx,
            SessionEvent::TransportTrouble(Arc::from(format!("会话文件里有一个这一版不认识的键，已忽略：{key}"))),
        );
    }

    let fs = match ToolExecutor::new(&spec.tools_root) {
        Ok(fs) => fs,
        Err(e) => {
            let _ = ready_tx.send(Err(format!(
                "内置工具初始化失败（root={}）: [{}] {}",
                spec.tools_root.display(),
                e.code,
                e.message
            )));
            return;
        }
    };

    let events_for_callback = events_tx.clone();
    let mut ctx = RunnerCtx::new(
        Arc::clone(&spec.provider),
        Arc::clone(&spec.client),
        spec.endpoint.clone(),
        spec.api_key.clone(),
        fs,
        spec.tools.build(),
        spec.system.clone(),
        SessionConfig { model: Arc::clone(&spec.model), temperature: None, max_tokens: None, context_window: None },
        store,
        // `new` 收的这条不带归属的回调不用——034 换 `with_agent_events`（下面），
        // 跟 `agent_cli::main` 装配 `RunnerCtx` 的手法同一个模式（`with_agent_events`
        // **替换**整条事件出口，见 `RunnerCtx::with_agent_events` 文档）。
        Box::new(|_| {}),
    )
    .with_agent_events(Box::new(move |ev: AgentEvent| {
        let _ = events_for_callback.send(Frame { agent: ev.agent, event: ev.event.into() });
    }));
    if let Some(timeout) = spec.provider_timeout {
        ctx = ctx.with_provider_timeout(timeout);
    }
    if let Some(every) = spec.snapshot_every {
        ctx = ctx.with_snapshot_every(every);
    }

    // 恢复之后必调——不然 `persist::sync` 会把 `Session::restore` 灌回来的旧
    // 条目当新条目重新 append 一遍（`agent_runtime::persist::seed_after_recover`
    // 文档「真 bug」一节）。对全新会话是无害的空操作，不需要在这里分支判断。
    agent_runtime::persist::seed_after_recover(&mut ctx, &session);

    let cancel = ctx.cancel_flag();
    if ready_tx.send(Ok(cancel)).is_err() {
        // opener 那边已经不要这个握手结果了（比如它自己被取消/超时放弃了）——
        // 没有订阅者、没有命令来源，继续跑下去没有意义。
        return;
    }
    drop(ready_tx);

    for cmd in rx.iter() {
        match cmd {
            Command::Input(text) => commands::handle_input(&mut session, &mut ctx, &events_tx, &text),
            Command::Undo { granularity, force } => commands::handle_undo(&mut session, &mut ctx, &events_tx, granularity, force),
            Command::Redo => commands::handle_redo(&mut session, &mut ctx, &events_tx),
            // 防御性第二道闸：正常路径下这个变体不会出现在队列里，
            // 见 `crate::command` 模块文档。
            Command::Cancel => ctx.cancel_flag().store(true, std::sync::atomic::Ordering::Relaxed),
            Command::Shutdown => break,
        }
    }
}
