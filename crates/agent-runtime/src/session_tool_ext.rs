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
//! 外部扩展/独测不该拿到这一整套：[`SessionToolFn`] 收窄到「拿 `Session` 手套
//! + 这次调用的入参」——照 `status_tool::intercept` 的先例，纯读会话状态、算
//! 一个结果就够绝大多数扩展工具用了。返回值 `Ok` 是给模型看的 tool_result 正文，
//! `Err` 是拒绝文案（决策 20：不 panic、不卡这一轮，让模型自己收敛）。
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
//!   （`set_max_turns`/`mark_irreversible`/`spawn_child`/……）已经是唯一合法的写
//!   入口——它们内部都经 `commit`/`commit_as` 落一条 journaled 的 `Entry`，扩展
//!   不需要（也没有办法）绕过去直接碰 store。
//!
//! # [`adapt`] 替扩展多做的两件事：可见性 + 红线 6
//!
//! [`SessionToolFn`] 只关心「读 Session、算结果」，够不着 `ctx` 本身——[`adapt`]
//! 把两件跟具体扩展无关、但每次工具调用都该有的事在调用户闭包**之前**做掉
//! （[`announce`]，逐字对齐既有截获的既有形状）：
//!
//! 1. **`ToolExecuting` 可见性**：跟既有四条截获（以及 `tool_exec::execute`）
//!    一样，先发一条正在执行的通报，CLI/面板才看得见这次调用。
//! 2. **红线 6 的屏障登记**：注册名的可逆性由 `ToolTable::snapshot`（名字规则或
//!    宿主注入映射）决定，`Irreversible` 的必须在**派发这一刻**登记
//!    （`Session::mark_irreversible`），跟 `dispatch.rs` 里「027 发起时快照」
//!    那段规矩相同——这条截获绕开了那段手写代码，账不能因此漏记。
//!
//! [`adapt`] 因此只剩「还两笔账 + 调用户函数 + 落地 tool_result」——当场回写、
//! 无 Pending、无在飞凭据、无 entry 要同步，跟 `status_tool::intercept` 逐字
//! 同一个形状。既有四条**不经过**这里：它们自己已经还了这两笔账（迁移前
//! `dispatch.rs` 手工 if 链直接调用它们时就是这样），见
//! `crate::intercept_registry` 模块文档「`dispatch` 不做任何包装」。

use std::sync::Arc;

use agent_core::{AgentId, Reversibility, Session, ToolCallId};
use serde_json::Value;

use crate::ctx::RunnerCtx;
use crate::dispatch::Dispatched;
use crate::event::RunnerEvent;
use crate::intercept_registry::{InterceptArgs, InterceptFn};
use crate::reply;

/// 公开层：扩展/独测吃这个。收窄到「拿 `Session` 手套 + 这次调用的入参」——机制
/// 上够不着 `Subtree`/`CompactSlots`/`IoBus`，那三个只有内部层截获用得到。
///
/// 返回值 `Ok` 是给模型看的 tool_result 正文，`Err` 是拒绝文案（决策 20：不
/// panic、不卡这一轮，让模型自己收敛）。
pub type SessionToolFn =
    Box<dyn Fn(&mut Session, &AgentId, &Value) -> Result<Arc<str>, Arc<str>> + Send + Sync>;

/// 可见性 + 红线 6 的屏障登记——[`adapt`] 替 [`SessionToolFn`] 扩展代还的那两笔
/// 账（模块文档「`adapt` 替扩展多做的两件事」）。
fn announce(
    session: &mut Session,
    ctx: &mut RunnerCtx,
    agent: &AgentId,
    call_id: ToolCallId,
    name: &str,
    input: Arc<Value>,
) {
    let request = ctx.tools.snapshot(name, input);
    if matches!(request.reversibility, Reversibility::Irreversible) {
        session.mark_irreversible(call_id.clone());
    }
    ctx.emit(agent, RunnerEvent::ToolExecuting { call_id, request });
}

/// [`SessionToolFn`] → [`InterceptFn`] 的适配器：先 [`announce`] 还两笔账，再调
/// 用户闭包 + 落地 tool_result。
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
        announce(session, ctx, agent, call_id.clone(), &name, Arc::clone(input));
        match f(session, agent, input) {
            Ok(body) => reply::ok(ctx, agent, call_id, epoch, &name, body.to_string()),
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
        self.session_tools.register(Arc::clone(&name), adapt(name, f));
    }
}
