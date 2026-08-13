//! `web:` 工具在浏览器里的执行点：**先查内建，没命中才交给页面装的工具回调**。
//!
//! 内建就是 [`crate::tools`] 声明的那几条——谁真的去读 `document.title` /
//! `location.href`，以及验收脚手架 `web:source/echo` 怎么原样回显。
//!
//! # 这条路跟 M10 的远端工具是**同一条**，只是两端住在同一个进程里
//!
//! `dispatch` 看到 `location.is_remote()` 且工具表声明了它 → 登记一个等待槽、
//! 发一条 `ToolExecuting` 给宿主、**挂起本轮**（`Dispatched::Nothing`）。宿主
//! 执行完把结果交回去，泵从那里继续——普通工具走
//! `agent_runtime::resolve_remote_tool_async`，`web:source/` 前缀的
//! transient-source 工具走 `agent_runtime::submit_remote_tool_result_async`
//! （124：分流理由见 [`crate::turn`] 模块文档）。server 形态下这两步之间隔着
//! 一次 HTTP 往返，浏览器形态下隔着一次函数调用——**能力注入链路一行没改**，
//! 这正是 114 验收第二条要证明的事（111 决策：同进程只是让它更快）。
//!
//! `tool_claim` 的 CAS / epoch / `sweep()` 照旧留着不动（111 §顺带消解的两件事：
//! 同进程下它们永远不触发，但 server 形态还要用，**不要因此删掉**）。
//!
//! # 派发顺序：内建优先，回调兜底（121）
//!
//! [`execute`] 先把三条内建名字全查一遍，**一条都没命中**才把这次调用交给页面装的
//! 回调（[`crate::callback::invoke_tool`]）。落到兜底那一支的名字，就是 122 里页面
//! 用 `capabilities_json.tools` 声明进来的那一段——**内建那三条页面连声明都不许**
//! （撞名在 [`crate::tools::declare`] 就被拒），所以这条顺序不可能被一份声明绕过。
//! 顺序反过来——回调优先——页面就能悄悄
//! 劫持一条已声明工具的语义：`AgentHost::tool_table_json()` 描述的东西和真正执行的
//! 东西对不上，**而且不报错**。模型按描述用工具，拿回来的却是别的东西，症状会以
//! 「模型行为莫名其妙」的形式出现在离这里很远的地方。
//!
//! 分流（普通 / transient-source）在 [`crate::turn`]，不在这里。所以回调对**两条路
//! 都生效**：页面将来声明一条 `web:source/vision`（130），它照样落到这个兜底分支，
//! 只是入参是认领后的真值、结果会被 transient-source 策略遮进历史。
//!
//! # 未知工具名与回调失败都走 `Failure` 而不是 panic
//!
//! 能走到这里的名字一定在工具表里（`dispatch` 那道闸拦过）。所以「既不是内建、
//! 页面又没装回调」= 页面声明了一条自己不实现的工具，是配置错，不是运行时崩溃：
//! 返回一条 `is_error` 的结果，模型看到会自纠，panic 会带走整个页面。回调
//! reject / 同步 throw / 返回非字符串，同理——三种都在
//! [`crate::callback::invoke_tool`] 里被翻成一条正文，一种都不 panic。

use agent_runtime::{RemoteToolOutput, RemoteToolWaiting};

use crate::callback;
use crate::tools::{PAGE_TITLE_TOOL, PAGE_URL_TOOL, SOURCE_ECHO_TOOL};

/// 执行一次宿主工具。**签名是异步的**——`drain_host_tools`/`drain_transient_source`
/// 那条 await 链上不该有同步点。三条内建读/回显的都是当场就有的值，没有 IO 可等，
/// 所以那几个分支的实现体仍是同步的，只是包在一个立刻就绪的 future 里——**不要为了
/// 「对称」去 await 一个立刻 resolve 的 Promise**，那只是多一次微任务调度。
/// 真正会等的是最后那条兜底分支：页面的 Promise 想挂多久挂多久（「等不到会怎样」
/// 归 123，本条只保证「能等」）。
///
/// 124：调用方对 `SOURCE_ECHO_TOOL` 传进来的 `waiting.request.input` 必须是
/// `claim_remote_tool` 认领之后拿到的**真入参**，不能是等待槽投影里那份已经被
/// dispatch 脱敏成占位符的版本——脱敏只对历史/prompt 生效，执行这一步必须看见
/// 真值，否则「原样返回入参」这条验收脚手架什么都验不出来。分流逻辑在
/// [`crate::turn`]，这里只管执行，不管这份 `input` 是从哪条路径来的。
pub(crate) async fn execute(waiting: &RemoteToolWaiting) -> RemoteToolOutput {
    match &*waiting.request.tool {
        PAGE_TITLE_TOOL => match document_title() {
            Some(title) => RemoteToolOutput::Success(title),
            None => RemoteToolOutput::Failure("拿不到 document：宿主不是页面主线程".to_string()),
        },
        PAGE_URL_TOOL => match location_href() {
            Some(href) => RemoteToolOutput::Success(href),
            None => RemoteToolOutput::Failure("拿不到 location：宿主不是页面主线程".to_string()),
        },
        SOURCE_ECHO_TOOL => RemoteToolOutput::Success(waiting.request.input.to_string()),
        // 内建都没命中：交给页面。`None` = 页面根本没装回调，措辞跟 121 之前那条
        // `_ =>` 分支逐字一样——对模型来说「这个宿主没有实现它」确实是同一件事。
        other => match callback::invoke_tool(other, &waiting.request.input).await {
            Some(Ok(content)) => RemoteToolOutput::Success(content),
            Some(Err(message)) => RemoteToolOutput::Failure(message),
            None => RemoteToolOutput::Failure(format!("这个宿主没有实现工具 `{other}`")),
        },
    }
}

fn document_title() -> Option<String> {
    Some(web_sys::window()?.document()?.title())
}

fn location_href() -> Option<String> {
    web_sys::window()?.location().href().ok()
}
