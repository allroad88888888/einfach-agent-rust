//! 146：`SessionToolFn`——截获式扩展工具的**公开层**签名，以及它怎么适配进
//! [`crate::intercept_registry`] 那张表。机制本身（自借用陷阱、撞名判据、
//! `dispatch` 怎么查表调用）住那个文件，这里只管「扩展作者写的闭包长什么样、
//! 怎么变成表要的形状」——一个文件一件事（红线 9）。
//!
//! # 为什么收窄，收窄到什么程度
//!
//! [`InterceptArgs`]（内部层）拿到的是 `dispatch::run_effect` 的全部入参面：
//! `Session`、`RunnerCtx`、`Subtree`、`CompactSlots`、`IoBus`……这一整套只有
//! `agent-runtime` 自己的既有四条截获（`crate::builtin_intercepts`，147 迁移
//! 进来的 spawn/collect/status/skill-read）用得上——它们要读写 `Subtree`、
//! spawn 后台模式还要一次产两个事件（`Dispatched::Events`）。
//!
//! 外部扩展/独测不该拿到这一整套：[`SessionToolFn`] 收窄到「拿 `Session` 手套 +
//! 这次调用的入参」——照 `status_tool::intercept` 的先例，纯读会话状态、算
//! 一个结果就够绝大多数扩展工具用了。返回值 `Ok` 是给模型看的 tool_result 正文
//! **加上这次调用在外部世界留下了什么**（201 的 [`Aftermath`]），`Err` 是拒绝文案
//! （决策 20：不 panic、不卡这一轮，让模型自己收敛）。
//!
//! # 纪律（机制不强制，如实写）：截获闭包的读写边界
//!
//! [`SessionToolFn`] 拿到的是**整个** `&mut Session`——机制上什么都读得到、写得到，
//! 但纪律要求：
//!
//! - **读按调用者的后代收窄**（红线 10）：照 `status_tool::observe` 的先例，
//!   `Session::agent_tree()` 给的是权威的整棵树，扩展要自己按 `agent` 参数过滤到
//!   「调用者能看到的那一段」，不能直接把整棵树喂给模型——那是把红线 10 挡的
//!   横读后门直接开在扩展层。
//! - **写只走 `Session` 的 command 面**（红线 2）：`Session` 暴露的公开方法
//!   （`set_max_turns`/`mark_no_undo`/`spawn_child`/……）已经是唯一合法的写
//!   入口——它们内部都经 `commit`/`commit_as` 落一条 journaled 的 `Entry`，扩展
//!   不需要（也没有办法）绕过去直接碰 store。
//!
//! # [`adapt`] 替扩展多做的两件事：可见性 + 记账
//!
//! [`SessionToolFn`] 只关心「读 Session、干活、算结果」，够不着 `ctx` 本身——
//! [`adapt`] 把两件跟具体扩展无关、但每次工具调用都该有的事做掉：
//!
//! 1. **`ToolExecuting` 可见性**（[`announce`]，在调用户闭包**之前**）：跟既有四条
//!    截获（以及 `tool_exec::execute`）一样，先发一条正在执行的通报，CLI/面板才
//!    看得见这次调用。
//! 2. **撤销记账**（[`record`]，在用户闭包**返回之后**）：把执行体交代的
//!    [`Aftermath`] 翻译成 core 那一位（`mark_hooked` / `mark_no_undo` / 都不标），
//!    交回来的还原函数进 [`crate::undo_hook`] 的表。
//!
//! ## 记账为什么从「派发前」挪到「返回后」（201，决策 199 §一）
//!
//! 199 之前这一笔是在 [`announce`] 里还的：查 `ToolTable::snapshot` 拿到注册时
//! 声明的 `Reversibility`，`Irreversible` 就当场 `mark_no_undo`。**依据是声明，
//! 时机是派发前**——两件事现在都变了：
//!
//! - **依据**：可逆性从此是每次调用的属性，只有执行体自己知道这次到底碰没碰外部
//!   世界、交不交得出逆（199 §一：`fs/write` 建新文件 / 覆盖旧文件 / 写失败，
//!   同一个工具三种还原方式）。注册时那个枚举表达不了，也不再是任何行为的依据。
//! - **时机**：`dispatch.rs` 那句「必须在派发这一刻登记」防的是**异步**工具
//!   （起飞之后进程崩在半路）。这条路上不存在那个窗口：`f(..)` 是同步跑完的，
//!   拿到返回值和落地 tool_result 之间没有任何 await、没有任何 IO。而 core 那半边
//!   本来就是「结果落地那条 entry 才带上这一位」，所以晚这几微秒登记，落盘的字节
//!   逐字节相同。
//!
//! ## `Err`（拒绝）这条路按「什么都没碰」记账
//!
//! 返回 `Err` = 决策 20 的拒绝文案：这次调用**没干成**，模型看着 `is_error` 自纠。
//! 所以不标任何位（`StateOnly`）——把拒绝也算成屏障的话，一个入参写错的调用就能
//! 让整轮撤不掉。**代价说清楚**：执行体「做了一半才失败」时不能走 `Err`，那时它
//! 该返回 `Ok((失败说明, Aftermath::Irreversible))` 或者交回一个只收拾做了那一半的
//! [`Aftermath::Undo`]——`Err` 的语义是「没碰」，不是「失败」。
//!
//! [`adapt`] 因此只剩「通报 + 调用户函数 + 记账 + 落地 tool_result」——当场回写、
//! 无 Pending、无在飞凭据、无 entry 要同步，跟 `status_tool::intercept` 逐字
//! 同一个形状。既有四条**不经过**这里：它们自己已经还了这些账（迁移前
//! `dispatch.rs` 手工 if 链直接调用它们时就是这样），见
//! `crate::intercept_registry` 模块文档「`dispatch` 不做任何包装」。

use std::sync::Arc;

use agent_core::{AgentId, Session, ToolCallId};
use serde_json::Value;

use crate::ctx::RunnerCtx;
use crate::dispatch::Dispatched;
use crate::event::RunnerEvent;
use crate::intercept_registry::{InterceptArgs, InterceptFn};
use crate::reply;
use crate::undo_hook::Aftermath;

/// 公开层：扩展/独测吃这个。收窄到「拿 `Session` 手套 + 这次调用的入参」——机制
/// 上够不着 `Subtree`/`CompactSlots`/`IoBus`，那三个只有内部层截获用得到。
///
/// 返回值 `Ok` 是**两样东西**：给模型看的 tool_result 正文，以及这次调用在外部
/// 世界留下了什么（[`Aftermath`]，决策 199 §一）。`Err` 是拒绝文案（决策 20：不
/// panic、不卡这一轮，让模型自己收敛），按「什么都没碰」记账（模块文档）。
pub type SessionToolFn = Box<
    dyn Fn(&mut Session, &AgentId, &Value) -> Result<(Arc<str>, Aftermath), Arc<str>> + Send + Sync,
>;

/// 可见性：发一条 `ToolExecuting`，CLI/面板才看得见这次调用。
///
/// 这里**不再**碰撤销记账（201 把那一笔挪到了 [`record`]，理由见模块文档）。
/// `request` 里那个 `reversibility` 从此只是给人看的标签（199 §八）。
fn announce(
    ctx: &mut RunnerCtx,
    agent: &AgentId,
    call_id: ToolCallId,
    name: &str,
    input: Arc<Value>,
) {
    let request = ctx.tools.snapshot(name, input);
    ctx.emit(agent, RunnerEvent::ToolExecuting { call_id, request });
}

/// 执行体交代的事实 → core 的那一位 + runtime 的钩子表（决策 199 §一 的 1:1 翻译）。
///
/// **翻译在宿主侧做**：`Aftermath` 是运行时词汇（工具交代的事实），`Undoability`
/// 是账本词汇（这条 entry 的记账），两个类型不合并——core 不认识 `UndoFn`（红线 7）。
///
/// 函数本体进 [`crate::undo_hook`] 的**暂存区**而不是直接进表：这一刻 entry 还没
/// 落地（tool_result 事件要等泵下一圈 `session.step` 才变成一条 entry），`seq` 还
/// 不存在。那边的模块文档解释了两步登记的全过程。
fn record(session: &mut Session, ctx: &mut RunnerCtx, call_id: &ToolCallId, aftermath: Aftermath) {
    match aftermath {
        // 没碰外部世界 → 一位都不标 → `Undoability::StateOnly`。**这跟
        // `Irreversible` 的区别是 199 全部的要点**：不是「碰了但撤不回」。
        Aftermath::Nothing => {}
        Aftermath::Undo(undo) => {
            session.mark_hooked(call_id.clone());
            ctx.undo_hooks.stage(call_id.clone(), undo);
        }
        Aftermath::Irreversible => session.mark_no_undo(call_id.clone()),
    }
}

/// [`SessionToolFn`] → [`InterceptFn`] 的适配器：[`announce`] → 调用户闭包 →
/// [`record`] → 落地 tool_result。
pub(crate) fn adapt(name: Arc<str>, f: SessionToolFn) -> InterceptFn {
    Box::new(move |args: InterceptArgs<'_>| -> Dispatched {
        let InterceptArgs {
            session,
            ctx,
            agent,
            call_id,
            input,
            epoch,
            ..
        } = args;
        announce(ctx, agent, call_id.clone(), &name, Arc::clone(input));
        match f(session, agent, input) {
            Ok((body, aftermath)) => {
                record(session, ctx, &call_id, aftermath);
                reply::ok(ctx, agent, call_id, epoch, &name, body.to_string())
            }
            Err(message) => reply::refuse(ctx, agent, call_id, epoch, &name, message.to_string()),
        }
    })
}

impl RunnerCtx {
    /// 注册一个截获式扩展工具（146，决策 29 的正门）。撞名判据见
    /// `RunnerCtx::registrable`（`crate::intercept_registry`）；`f` 的读写纪律见
    /// 模块文档「纪律」那节——机制不强制，注释立字据。
    pub fn register_session_tool(&mut self, name: Arc<str>, f: SessionToolFn) {
        if let Err(reason) = self.registrable(&name) {
            debug_assert!(false, "register_session_tool(`{name}`) 被拒：{reason}");
            return;
        }
        self.session_tools
            .register(Arc::clone(&name), adapt(name, f));
    }
}
