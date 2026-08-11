//! `web:` 工具在浏览器里的执行点——[`crate::tools`] 声明的那两条，谁真的去读
//! `document.title` / `location.href`。
//!
//! # 这条路跟 M10 的远端工具是**同一条**，只是两端住在同一个进程里
//!
//! `dispatch` 看到 `location.is_remote()` 且工具表声明了它 → 登记一个等待槽、
//! 发一条 `ToolExecuting` 给宿主、**挂起本轮**（`Dispatched::Nothing`）。宿主
//! 执行完调 `agent_runtime::resolve_remote_tool` 把结果交回去，泵从那里继续。
//! server 形态下这两步之间隔着一次 HTTP 往返，浏览器形态下隔着一次函数调用——
//! **能力注入链路一行没改**，这正是 114 验收第二条要证明的事（111 决策：同进程
//! 只是让它更快）。
//!
//! `tool_claim` 的 CAS / epoch / `sweep()` 照旧留着不动（111 §顺带消解的两件事：
//! 同进程下它们永远不触发，但 server 形态还要用，**不要因此删掉**）。
//!
//! # 未知工具名走 `Failure` 而不是 panic
//!
//! 能走到这里的名字一定在工具表里（`dispatch` 那道闸拦过），所以 `_ =>` 分支
//! 结构上不可达。但它仍然返回一条 `is_error` 的结果而不是 panic：模型看到
//! `is_error` 会自纠，panic 会带走整个页面。

use agent_runtime::{RemoteToolOutput, RemoteToolWaiting};

use crate::tools::{PAGE_TITLE_TOOL, PAGE_URL_TOOL};

/// 执行一次宿主工具。**同步**——两个工具读的都是当场就有的值，没有 IO 可等。
pub(crate) fn execute(waiting: &RemoteToolWaiting) -> RemoteToolOutput {
    match &*waiting.request.tool {
        PAGE_TITLE_TOOL => match document_title() {
            Some(title) => RemoteToolOutput::Success(title),
            None => RemoteToolOutput::Failure("拿不到 document：宿主不是页面主线程".to_string()),
        },
        PAGE_URL_TOOL => match location_href() {
            Some(href) => RemoteToolOutput::Success(href),
            None => RemoteToolOutput::Failure("拿不到 location：宿主不是页面主线程".to_string()),
        },
        other => RemoteToolOutput::Failure(format!("这个宿主没有实现工具 `{other}`")),
    }
}

fn document_title() -> Option<String> {
    Some(web_sys::window()?.document()?.title())
}

fn location_href() -> Option<String> {
    web_sys::window()?.location().href().ok()
}
