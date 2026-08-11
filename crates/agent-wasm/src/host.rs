//! [`AgentHost`]：暴露给页面 JS 的全部接口。**五件事**——建会话、发一句话、
//! 拿流式增量、取消、切会话。
//!
//! # 借用纪律（这个文件唯一真正微妙的地方）
//!
//! `run_turn` 要 `&mut Session` + `&mut RunnerCtx`，而它是一条很长的 await 链。
//! 于是 `send()` 会**在整轮对话期间**持有 `live` 的可变借用。取消必须在这期间
//! 生效，所以取消标志（`Arc<AtomicBool>`，`RunnerCtx::cancel_flag()` 给的那一份）
//! 单独放在另一个 `RefCell` 里：`cancel()` 只碰它，碰不到 `live`。
//!
//! 由此得到两条对页面的约定，**违反会 panic 而不是静默出错**（这是好事，
//! 静默的重入会变成一个查不出来的状态错乱）：
//!
//! 1. 上一轮 `send()` 的 Promise 没 settle 之前，不要再调 `send()`/`open_session()`；
//! 2. 事件回调里不要回头调 `send()`/`open_session()`——回调正是在那轮借用之内被
//!    调用的。回调里只读、只画。
//!
//! `cancel()`、`tool_table_json()`、`key_len()` 不在此列：它们都不碰 `live`。
//!
//! # key 只从使用者来
//!
//! 构造这个类型的唯一入口收一份页面给的配置 JSON（[`crate::config`]），**代码里
//! 没有任何默认 key，也没有任何地方把 key 打印出来**（111 契约第 4 条）。

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use agent_runtime::AgentEvent;
use wasm_bindgen::prelude::*;

use crate::assemble::{self, Live};
use crate::{config::HostConfig, events, history, session_id, tools, turn};

/// 页面手里的那个对象。
#[wasm_bindgen]
pub struct AgentHost {
    inner: Rc<Inner>,
}

struct Inner {
    config: HostConfig,
    /// 建好就不变——`tool_table_json()` 因此不需要借 `live`（见模块文档的借用
    /// 纪律），页面在任何时刻都能取到它做字节比对。
    tool_table_json: String,
    /// 页面装的事件回调。`Rc<RefCell<_>>` 而不是直接存进 `RunnerCtx`：切会话时
    /// `RunnerCtx` 整个换掉，回调不该跟着掉。
    on_event: Rc<RefCell<Option<js_sys::Function>>>,
    live: RefCell<Option<Live>>,
    /// 当前会话的取消标志。见模块文档——它必须能在 `live` 被借着的时候被翻。
    cancel: RefCell<Option<Arc<AtomicBool>>>,
}

#[wasm_bindgen]
impl AgentHost {
    /// 收一份页面给的配置：
    /// `{"provider":"deepseek|kimi|glm","base_url":"…","model":"…","api_key":"…"}`。
    #[wasm_bindgen(constructor)]
    pub fn new(config_json: &str) -> Result<AgentHost, JsValue> {
        let config = HostConfig::parse(config_json).map_err(js_error)?;
        // 名字不认识就当场报，不要等第一次请求才发现 endpoint 和编码对不上。
        config.adapter().map_err(js_error)?;
        Ok(AgentHost {
            inner: Rc::new(Inner {
                tool_table_json: tools::tool_table_json(&tools::browser_tool_table()),
                config,
                on_event: Rc::new(RefCell::new(None)),
                live: RefCell::new(None),
                cancel: RefCell::new(None),
            }),
        })
    }

    /// 装一条事件回调：`handler(jsonString)`。形状见 `events.rs`。
    #[wasm_bindgen(js_name = onEvent)]
    pub fn on_event(&self, handler: js_sys::Function) {
        *self.inner.on_event.borrow_mut() = Some(handler);
    }

    /// 这个宿主给模型的工具表，原样序列化。验收第三条（刷新前后逐字节相同）与
    /// 第五条（没有 `srv:`）的证据面，见 [`crate::tools`] 模块文档。
    #[wasm_bindgen(js_name = toolTableJson)]
    pub fn tool_table_json(&self) -> String {
        self.inner.tool_table_json.clone()
    }

    /// key 的**长度**，不是 key。页面横幅只许显示这个。
    #[wasm_bindgen(js_name = keyLen)]
    pub fn key_len(&self) -> usize {
        self.inner.config.key_len()
    }

    /// 当前打开的会话 id。没开就是 `undefined`。
    #[wasm_bindgen(js_name = sessionId)]
    pub fn session_id(&self) -> Option<String> {
        self.inner
            .live
            .borrow()
            .as_ref()
            .map(|live| live.id.clone())
    }

    /// 开一个会话（同一个 id 再开一次 = 从 IndexedDB 把它接回来；换个 id =
    /// 切会话）。Promise 结果是**重放出来的历史** JSON，页面据此重画。
    #[wasm_bindgen(js_name = openSession)]
    pub fn open_session(&self, id: String) -> js_sys::Promise {
        let inner = Rc::clone(&self.inner);
        wasm_bindgen_futures::future_to_promise(async move {
            let id = session_id::validate(&id).map_err(js_error)?.to_string();
            let live = assemble::open(
                id,
                &inner.config,
                event_sink(Rc::clone(&inner.on_event)),
                store_error_sink(Rc::clone(&inner.on_event)),
            )
            .await
            .map_err(js_error)?;
            *inner.cancel.borrow_mut() = Some(live.ctx.cancel_flag());
            let replayed = history::to_json(&live.session);
            *inner.live.borrow_mut() = Some(live);
            Ok(JsValue::from_str(&replayed))
        })
    }

    /// 说一句话，跑到这一轮结束。Promise 结果是一份 JSON：
    /// `{"status":"…","cancelledTurn":"…"|null}`；流式增量走事件回调，不走这个
    /// 返回值。`cancelledTurn` 只在这一轮被取消时非空，说的是「被丢弃的半轮
    /// 到底丢没丢干净」（撞上不可逆屏障时会留下，用户该知道）。
    pub fn send(&self, text: String) -> js_sys::Promise {
        let inner = Rc::clone(&self.inner);
        wasm_bindgen_futures::future_to_promise(async move {
            let mut guard = inner.live.borrow_mut();
            let live = guard
                .as_mut()
                .ok_or_else(|| js_error("还没打开会话：先调 openSession(id)"))?;
            // `Err` 是 M12 的 transient-source 出口（见 `turn::run`：这个宿主的
            // 工具表里结构上不可达）。真出现就是一条给页面的错误，不是一个假的
            // 终态——`js_error` 会让那个 Promise reject。
            let outcome = turn::run(&mut live.session, &mut live.ctx, &text)
                .await
                .map_err(|failure| js_error(&format!("{failure:?}")))?;
            let payload = serde_json::json!({
                "status": format!("{:?}", outcome.status),
                "cancelledTurn": outcome.cancelled_turn.map(|report| format!("{report:?}")),
            });
            Ok(JsValue::from_str(&payload.to_string()))
        })
    }

    /// 取消正在飞的这一轮。**不碰 `live`**，所以可以在 `send()` 的 Promise 还
    /// 没 settle 时调（那正是它唯一有意义的时机）。
    pub fn cancel(&self) {
        if let Some(flag) = self.inner.cancel.borrow().as_ref() {
            flag.store(true, Ordering::Relaxed);
        }
    }

    /// 当前会话的历史，形状同 `openSession` 的结果。
    #[wasm_bindgen(js_name = historyJson)]
    pub fn history_json(&self) -> String {
        match self.inner.live.borrow().as_ref() {
            Some(live) => history::to_json(&live.session),
            None => "[]".to_string(),
        }
    }
}

/// runner 事件 → 页面回调。**先把 `Function` 克隆出来再放掉借用**：回调里如果
/// 调了 `onEvent()` 换回调，持有借用调过去就是一次 `already borrowed` panic。
fn event_sink(handler: Rc<RefCell<Option<js_sys::Function>>>) -> Box<dyn FnMut(AgentEvent)> {
    Box::new(move |event: AgentEvent| {
        let Some(callback) = handler.borrow().clone() else {
            return;
        };
        let _ = callback.call1(&JsValue::NULL, &JsValue::from_str(&events::to_json(&event)));
    })
}

/// 持久化后端的错误出口（`SessionStore` 是 fire-and-forget，失败只经这条路上报）。
/// 走跟事件同一条回调，页面因此只需要接一个 sink。
fn store_error_sink(handler: Rc<RefCell<Option<js_sys::Function>>>) -> impl Fn(String) + 'static {
    move |detail: String| {
        let Some(callback) = handler.borrow().clone() else {
            return;
        };
        let payload = serde_json::json!({ "type": "store_error", "detail": detail }).to_string();
        let _ = callback.call1(&JsValue::NULL, &JsValue::from_str(&payload));
    }
}

fn js_error(message: impl AsRef<str>) -> JsValue {
    js_sys::Error::new(message.as_ref()).into()
}
