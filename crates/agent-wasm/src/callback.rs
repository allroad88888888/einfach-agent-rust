//! 页面装进来的那几个 JS 函数：事件 sink、store 错误 sink、**工具执行回调**。
//!
//! # 为什么每一条都是 `Rc<RefCell<Option<Function>>>`，挂在 `AgentHost` 上
//!
//! 切会话时 [`crate::assemble::Live`]（`Session` + `RunnerCtx`）整个换掉，页面装的
//! 回调不该跟着掉——页面装一次就该一直有效。所以每一条都是一个 [`Slot`]：
//! `AgentHost` 自己持有它，装配线只借一份克隆，切会话动不到它。
//!
//! # 调回调之前必须先把 `Function` 克隆出来
//!
//! 三条路子全是「先克隆 `Function` → 放掉 `borrow()` → 再 call」。页面在事件回调里
//! 换回调（`onEvent(另一个函数)`）是合法用法，持着借用调过去就是一次
//! `already borrowed` panic。[`invoke_tool`] 更要紧：它之后要 `await`，
//! **借用绝不能跨过那个 await 点**。
//!
//! # 工具回调怎么走到 [`crate::host_tool`]：一个线程局部的「当前生效槽」
//!
//! `host_tool::execute` 是被 [`crate::turn`] 那条 await 链调的，链上**没有
//! `AgentHost`**——`turn::run` 只拿 `&mut Session` + `&mut RunnerCtx`。而回调
//! 又不能放进 `RunnerCtx`（上面第一节：切会话会把它带走）。于是这里留一个线程
//! 局部的槽：[`install_tool`] 登记，[`invoke_tool`] 取用。登记的是**同一个
//! [`Slot`] 的 `Rc` 克隆**，不是函数的副本——所以页面再调一次 `onToolCall`
//! 换函数，这里立刻就看得见，没有第二份真相。
//!
//! 线程局部在这里安全，理由是这个宿主结构上单线程（wasm 主线程；这个 crate 里
//! `Rc`/`RefCell` 满地都是，本来就不存在第二个线程）。**代价说清楚**：同一个页面
//! 建了两个 `AgentHost` 各装一条回调时，**后装的那条生效**——`www/index.html` 只建
//! 一个，真出现两个也只错「谁的回调生效」这一件事，不会静默错值。

use std::cell::RefCell;
use std::rc::Rc;

use agent_runtime::AgentEvent;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use crate::events;

/// 页面装的一个 JS 函数的存放处。见模块文档第一节。
pub(crate) type Slot = Rc<RefCell<Option<js_sys::Function>>>;

thread_local! {
    /// 当前生效的工具回调槽。见模块文档第三节——它是 `host_tool` 那条 await 链
    /// 够得着 `AgentHost` 的唯一途径。
    static ACTIVE_TOOL_SLOT: RefCell<Option<Slot>> = const { RefCell::new(None) };
}

/// 装一条工具回调，并把它的槽登记成「当前生效的那一条」。
pub(crate) fn install_tool(slot: &Slot, handler: js_sys::Function) {
    *slot.borrow_mut() = Some(handler);
    ACTIVE_TOOL_SLOT.with(|active| *active.borrow_mut() = Some(Rc::clone(slot)));
}

/// 把一次工具调用交给页面装的回调。
///
/// - `None` = 页面**根本没装**回调。调用方（[`crate::host_tool::execute`]）据此给出
///   「这个宿主没有实现工具 `…`」那条失败——区分「没装」和「装了但失败」是必要的：
///   前者是配置问题，后者是执行问题，措辞不该混。
/// - `Some(Ok(text))` = resolve 出来的字符串，当工具结果正文。
/// - `Some(Err(message))` = reject、同步 `throw`、或者 resolve 出来的不是字符串。
///   三种都是**一条 `is_error` 的工具结果**，不是 panic：模型看见 `is_error` 会自纠，
///   panic 会带走整个页面（[`crate::host_tool`] 模块文档那条理由，本条第一次真用上）。
pub(crate) async fn invoke_tool(
    name: &str,
    input: &serde_json::Value,
) -> Option<Result<String, String>> {
    // 借用在这一句里开始、在这一句里结束——下面要 await（页面的 Promise 可能挂
    // 很久），借用跨过 await 点就意味着页面在自己的回调里再调一次 `onToolCall`
    // 就 panic。
    let handler = ACTIVE_TOOL_SLOT.with(|active| -> Option<js_sys::Function> {
        active.borrow().as_ref()?.borrow().clone()
    })?;
    Some(call_tool(&handler, name, &input.to_string()).await)
}

/// `handler(name, inputJson) -> Promise<string>`，等它 settle。
async fn call_tool(
    handler: &js_sys::Function,
    name: &str,
    input_json: &str,
) -> Result<String, String> {
    let returned = handler
        .call2(
            &JsValue::NULL,
            &JsValue::from_str(name),
            &JsValue::from_str(input_json),
        )
        // 回调**同步**抛出来的（比如参数还没解析就 throw）跟 reject 同一个待遇：
        // 页面写的是不是 `async function` 不该改变失败的形状。
        .map_err(describe)?;
    // 页面直接返回字符串（没包 Promise）也照收：`Promise::resolve` 对 Promise 是
    // 原样返回，对别的值是包成一个立刻就绪的 Promise。这不是鼓励同步返回——契约
    // 仍然是 `Promise<string>`；只是一个能干净处理的形状不必留成失败路径。
    let settled = wasm_bindgen_futures::JsFuture::from(js_sys::Promise::resolve(&returned))
        .await
        .map_err(describe)?;
    // 非字符串按失败处理，**不 panic**：`as_string()` 对 number/object/undefined
    // 一律给 `None`。正文里不回显那个值本身——它可能是个巨大的对象，而这条正文
    // 会原样进模型的 tool_result。
    settled
        .as_string()
        .ok_or_else(|| format!("工具回调 `{name}` 没有返回字符串"))
}

/// JS 抛出来的东西 → 一行人话。`Error` 取 `message`（121 定死的），字符串原样，
/// 其余交给 wasm-bindgen 对 `JsValue` 的 `Debug`。
fn describe(thrown: JsValue) -> String {
    if let Some(error) = thrown.dyn_ref::<js_sys::Error>() {
        return String::from(error.message());
    }
    thrown.as_string().unwrap_or_else(|| format!("{thrown:?}"))
}

/// runner 事件 → 页面回调。**先把 `Function` 克隆出来再放掉借用**，理由见模块文档。
pub(crate) fn event_sink(handler: Slot) -> Box<dyn FnMut(AgentEvent)> {
    Box::new(move |event: AgentEvent| {
        let Some(callback) = handler.borrow().clone() else {
            return;
        };
        let _ = callback.call1(&JsValue::NULL, &JsValue::from_str(&events::to_json(&event)));
    })
}

/// 持久化后端的错误出口（`SessionStore` 是 fire-and-forget，失败只经这条路上报）。
/// 走跟事件同一条回调，页面因此只需要接一个 sink。
pub(crate) fn store_error_sink(handler: Slot) -> impl Fn(String) + 'static {
    move |detail: String| {
        let Some(callback) = handler.borrow().clone() else {
            return;
        };
        let payload = serde_json::json!({ "type": "store_error", "detail": detail }).to_string();
        let _ = callback.call1(&JsValue::NULL, &JsValue::from_str(&payload));
    }
}
