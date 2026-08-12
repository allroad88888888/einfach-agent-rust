//! actor 线程真正跑的东西：现造 `Session` + `RunnerCtx`（两者 `!Send`，只能在
//! 这个线程内部诞生，见 `super` 模块文档），握手告诉 `open()` 「起好了还是
//! 起失败了」，然后进入命令循环直到 `Shutdown` 或者队列的发送端被丢弃。

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tokio::sync::broadcast;

use agent_core::{AgentId, AgentTree, ExecutionProfileId, Session, SessionConfig};
use agent_runtime::{
    AgentEvent, ExecutionBinding, RemoteToolStatusSnapshot, RemoteToolWaiting, RunnerCtx,
};
use agent_tools::ToolExecutor;

use crate::event::{Frame, SessionEvent};
use crate::registry::OpenSpec;

use super::message::ActorMessage;
use super::{capabilities, commands, inbox, session_start};

/// 握手消息：`Ok(cancel)` = 起好了，`cancel` 是 [`RunnerCtx::cancel_flag`]
/// （[`crate::handle::SessionHandle::cancel`] 直接旁路写的那个原子标志）；
/// `Err(reason)` = 恢复/构造阶段就失败了，线程即将退出、从未进入命令循环。
pub(super) type ReadyMsg = Result<Arc<AtomicBool>, String>;

/// 广播一条不属于任何具体 agent 的事件——actor/连接级的事实（落盘 IO 麻烦、
/// 快照里认不出的键之类），标 [`AgentId::root`]（034：`crate::event::frame`
/// 模块文档同一条判据）。
fn emit_root(events_tx: &broadcast::Sender<Frame>, event: SessionEvent) {
    let _ = events_tx.send(Frame {
        agent: AgentId::root(),
        event,
    });
}

pub(super) fn run(
    spec: OpenSpec,
    execution_bindings: BTreeMap<ExecutionProfileId, ExecutionBinding>,
    rx: mpsc::Receiver<ActorMessage>,
    events_tx: broadcast::Sender<Frame>,
    ready_tx: mpsc::Sender<ReadyMsg>,
    tree: Arc<Mutex<AgentTree>>,
    pending_tools: Arc<Mutex<Vec<RemoteToolWaiting>>>,
    tool_status: Arc<Mutex<RemoteToolStatusSnapshot>>,
) {
    let agent = AgentId::root();
    let history_cap = spec.history_cap.unwrap_or(agent_core::DEFAULT_HISTORY_CAP);

    // 落盘 IO 层面的麻烦（打不开文件、写失败）借用「传输/IO 麻烦」这一类事件
    // 广播出去——`SessionEvent` 没有为它单独开变体：对客户端来说，「底层通道
    // 出了岔子，文本里有细节」是同一件事，不管这条通道是 HTTP 还是本地文件。
    let events_for_store_errors = events_tx.clone();
    let store = agent_runtime::open_backend(spec.store_path.clone(), move |e| {
        emit_root(
            &events_for_store_errors,
            SessionEvent::TransportTrouble(Arc::from(e.to_string())),
        );
    });

    // 160：`limits` 与 `history_cap` 同类，恢复不出来、得由宿主再说一遍——两个都从
    // 这一档的 `SessionTemplate` 取，恢复出来的会话于是和新建的拿到同一组数。
    let limits = spec.tools.spawn_limits().unwrap_or_default();
    let mut unknown_keys: Vec<String> = Vec::new();
    let recovered = agent_runtime::recover(
        store.as_ref(),
        agent.clone(),
        history_cap,
        limits,
        &mut |key| unknown_keys.push(format!("{key:?}")),
    );

    // `restored` = 这个会话是从日志里回放出来的（不是全新建的）。073 用它分辨
    // 「注入的声明从哪来」——**新建看这次请求，恢复看回放出来的状态**。
    let restored = matches!(recovered, Ok(Some(_)));
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
            // （`ToolTableSpec::spawn_limits` 文档）。
            //
            // 160 更正：这里曾经写着「恢复出来的会话带着它自己持久化过的配置」
            // ——`limits` 和 `history_cap` **都不持久化**，那句话对两者都不成立。
            // 两个都由宿主在上面的 `recover` 调用里再说一遍，新建与恢复同一组数。
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
    // 048：`GET /sessions/:id/agents` 读的共享单元格，在这里第一次写真实值——
    // `session` 这一刻已经落定（新建或者恢复完毕，含恢复出来的既有子 agent 树），
    // 这一行必须排在 `ready_tx.send(Ok(cancel))`（下面）之前：`open()` 只有在
    // 那次握手成功之后才会把 `SessionHandle` 交给调用方，调用方能看到这个
    // 单元格的那一刻，它已经是真实的初始快照，不会是 `actor::spawn` 造的空
    // 占位（`crate::actor` 模块文档）。
    *tree.lock().unwrap() = session.agent_tree();

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
            SessionEvent::TransportTrouble(Arc::from(format!(
                "会话文件里有一个这一版不认识的键，已忽略：{key}"
            ))),
        );
    }

    // 宿主注入的能力（062/064/073）：声明从哪来、怎么变成这个会话的工具表与 system
    // 段、恢复出来的会话为什么不接受新声明——整件事在 `super::capabilities`，那个
    // 文件的模块文档是三条 issue 结论的落点。`Err` = 第二道闸拦下了（第一道是路由层
    // 的 400 `session_has_history`）。
    let assembled = match capabilities::assemble(&spec, &session, restored) {
        Ok(assembled) => assembled,
        Err(reason) => {
            let _ = ready_tx.send(Err(reason));
            return;
        }
    };

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
    // s5：配了 vision 就把 `srv:vision/inspect` 的运行时注入 executor，并把工具
    // 追加进工具表。两者必须一起给——只注 executor 不声明工具，模型不知道有它；
    // 只声明不注入，调用会落在 `not_configured`。
    let fs = match &spec.vision {
        Some(vision) => fs.with_vision(vision.clone()),
        None => fs,
    };
    let mut tools = assembled.tools;
    if spec.vision.is_some() {
        tools = tools.with_vision_inspect();
    }

    let events_for_callback = events_tx.clone();
    let events_for_tree = events_tx.clone();
    let tree_for_callback = Arc::clone(&tree);
    let mut ctx = RunnerCtx::new(
        Arc::clone(&spec.provider),
        Arc::clone(&spec.client),
        spec.endpoint.clone(),
        spec.api_key.clone(),
        fs,
        // 部署期五档 + 这个会话专属的注入（工具排表尾、skill 两件在它之前），以及
        // 部署期的 system 段 + 常驻 skill 索引。两段都 per-session：跟着这个 actor
        // 线程生灭，别的 chatid 看不见（docs/HOST-CAPABILITIES.md §二）。什么都没
        // 声明时两者都是空操作，跟 062/064 之前逐字节相同。
        tools,
        assembled.system,
        SessionConfig {
            model: Arc::clone(&spec.model),
            temperature: None,
            max_tokens: None,
            // 110 前置：从这份 `OpenSpec`（`SessionTemplate::open_spec` 转发
            // 的 `context_window`）取，不是硬编码 `None`。
            context_window: spec.context_window,
        },
        store,
        // `new` 收的这条不带归属的回调不用——034 换 `with_agent_events`（下面），
        // 跟 `agent_cli::main` 装配 `RunnerCtx` 的手法同一个模式（`with_agent_events`
        // **替换**整条事件出口，见 `RunnerCtx::with_agent_events` 文档）。
        Box::new(|_| {}),
    );
    ctx = ctx
        .with_execution_bindings(execution_bindings)
        .with_agent_events(Box::new(move |ev: AgentEvent| {
            let _ = events_for_callback.send(Frame {
                agent: ev.agent,
                event: ev.event.into(),
            });
        }))
        // 048：树快照变化——独立回调，不走上面那条 `AgentEvent` 通道（048 issue
        // 范围条款 1：树是整棵状态的投影，不是 `RunnerEvent` 的第十个变体）。每次
        // `run_turn` 判定树变了都会调用它一次：先重写共享单元格（`GET .../agents`
        // 的数据源），再广播一帧标 root 的 `SessionEvent::AgentTree`（`emit_root`——
        // 跟 `Undo`/`SessionDied`/`Gap` 同一条「会话级事实标 root」的判据，
        // `crate::event::frame` 模块文档）。写单元格排在广播前面：真的有并发的
        // `GET` 请求跟这次广播打个照面，它读到的至少是这次广播里同一份新树，不会
        // 是旧值（两者其实没有严格的先后依赖，只是这个顺序更符合直觉）。
        .with_tree_events(Box::new(move |snapshot: AgentTree| {
            *tree_for_callback.lock().unwrap() = snapshot.clone();
            emit_root(&events_for_tree, SessionEvent::AgentTree(snapshot));
        }))
        // 072：远端等待槽变了——只重写共享单元格（`GET .../pending_tools` 的数据源），
        // **不广播帧**：这份投影是给「要不要执行」当判据的，不是时间线上的一件事；
        // `tool_executing` 那一帧已经在派活的同一行发过了（`dispatch` 的远端第五路）。
        // 回调在**槽变化的那一刻**被调（那一行的下一行就是广播），所以客户端拿着帧
        // 立刻来问，问到的必然已经是含这条调用的新投影——见 `SessionHandle::pending_tools`。
        .with_pending_remote_tools(Box::new(move |waiting: Vec<RemoteToolWaiting>| {
            *pending_tools.lock().unwrap() = waiting;
        }))
        // 092：完整状态投影同样直接写共享单元格。尤其是提交回执先 commit、再 ack、
        // 随后可能继续做 provider IO；GET 状态不能被那段网络等待堵在 actor 队列里。
        .with_remote_tool_status(Box::new(move |status: RemoteToolStatusSnapshot| {
            *tool_status.lock().unwrap() = status;
        }));
    if let Some(timeout) = spec.provider_timeout {
        ctx = ctx.with_provider_timeout(timeout);
    }
    if let Some(timeout) = spec.remote_tool_timeout {
        ctx = ctx.with_remote_tool_timeout(timeout);
    }
    if let Some(every) = spec.snapshot_every {
        ctx = ctx.with_snapshot_every(every);
    }

    // 恢复之后必调——不然 `persist::sync` 会把 `Session::restore` 灌回来的旧
    // 条目当新条目重新 append 一遍（`agent_runtime::persist::seed_after_recover`
    // 文档「真 bug」一节）。对全新会话是无害的空操作，不需要在这里分支判断。
    agent_runtime::persist::seed_after_recover(&mut ctx, &session);

    // 135：工具表装完之后跑一次开局工具，只在「都没有才建」这一支（`restored`
    // 是这三态判定的既有变量）。**必须排在 `seed_after_recover` 之后**（139 修的
    // 真 bug）：`maybe_run` 会给新会话追加一条 journaled 的 `prefix_init` entry；
    // 排在 `seed_after_recover` 之前，这条刚写的 entry 会被误判成「已经在盘上」，
    // 从此永远不被 `persist::sync` 真正落盘，重启即丢。`Err` 并进上面几处一样的
    // 「actor 启动阶段的失败」早退路径。
    if let Err(reason) = session_start::maybe_run(restored, &mut session, ctx.tools()) {
        let _ = ready_tx.send(Err(reason));
        return;
    }

    // 073/064：全新会话把这一次的声明**journaled 地写一次**（`Slot::HostTools` /
    // `Slot::HostSkills`），恢复时跟别的 primitive 一样自动回来。**必须排在
    // `seed_after_recover` 之后**，而且声明自成一轮——两条顺序的理由（都是踩出来的）
    // 见 `capabilities::record`。
    capabilities::record(&mut ctx, &mut session, &spec, restored);

    // JSONL 能恢复出 core 的 `ToolsPending`，但宿主的 claim/receipt/evidence 都只在
    // 旧进程内存里；绝不能凭那份旧槽重放工具，也不能让新 actor 因 ctx 没有等待表而
    // 裸 recv 永久挂住。恢复时直接把整轮取消成终态，清空共享等待投影并发出终态事件。
    if restored
        && (agent_runtime::has_unresolved_tool_calls(&session)
            || agent_runtime::recovered_transient_source_needs_fail_close(&session))
    {
        if let Err(failure) = agent_runtime::cancel_pending_remote_tools(&mut session, &mut ctx) {
            commands::emit_transient_source_failure(&events_tx, failure);
        }
    }

    let cancel = ctx.cancel_flag();
    if ready_tx.send(Ok(cancel)).is_err() {
        // opener 那边已经不要这个握手结果了（比如它自己被取消/超时放弃了）——
        // 没有订阅者、没有命令来源，继续跑下去没有意义。
        return;
    }
    drop(ready_tx);

    loop {
        let Some(message) = next_message(&rx, &mut session, &mut ctx, &events_tx) else {
            return;
        };
        if matches!(
            inbox::dispatch(message, &mut session, &mut ctx, &events_tx),
            inbox::LoopControl::Break
        ) {
            break;
        }
    }
}

/// 等下一条命令，**但不许无限期地等**（060）。
///
/// 远端工具（`web:`/`desk:`）派出去之后 `run_turn` 就返回 `ToolsPending`，控制权
/// 回到这条循环：正常路径靠 `POST /tool_result` 送一条 `Command::RemoteToolResult`
/// 进来，异常路径靠用户 `Cancel`。可「前端崩了 / 网关挂了 / 客户端根本没实现这个
/// 工具」这三种情况下**两条都不会来**——裸 `rx.recv()` 于是永远阻塞，会话永久停在
/// `ToolsPending` 且不报错。所以：有等待槽时至多等到最早的那条截止线，到点让
/// runtime 把它判失败（模型收到 `is_error` 自己收敛），回来接着等。
///
/// 没有等待槽时（绝大多数会话的绝大多数时间）走的还是裸 `recv()`——不轮询、不
/// 起定时器、一分钱开销不多付。`None` = 命令队列的发送端全没了，线程该退出了。
fn next_message(
    rx: &mpsc::Receiver<ActorMessage>,
    session: &mut Session,
    ctx: &mut RunnerCtx,
    events_tx: &broadcast::Sender<Frame>,
) -> Option<ActorMessage> {
    loop {
        let Some(deadline) = ctx.next_remote_deadline() else {
            return rx.recv().ok();
        };
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(cmd) => return Some(cmd),
            // 到点了：扫过期槽（`deadline` 那一条必然在里面，所以每一圈都有进展，
            // 不会空转），再回来等剩下的。
            Err(mpsc::RecvTimeoutError::Timeout) => {
                commands::handle_remote_tool_timeout(session, ctx, events_tx)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return None,
        }
    }
}
