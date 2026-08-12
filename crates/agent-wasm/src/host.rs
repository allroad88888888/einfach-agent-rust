//! [`AgentHost`]：暴露给页面 JS 的全部接口。**七件事**——建会话、发一句话、
//! 拿流式增量、取消、切会话、删会话、识图。
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
//! `cancel()`、`tool_table_json()`、`key_len()`、`inspect_image()` 不在此列：
//! 它们都不碰 `live`——识图不依赖任何已开的会话，页面不用先 `openSession`
//! 就能调（见 [`crate::vision`] 模块文档）。
//!
//! [`AgentHost::delete_session`] 碰 `live`，但它用 `try_borrow_mut` 把「撞上在飞的
//! 一轮」变成一次 **reject 而不是 panic**——那是个破坏性操作，页面上那个按钮随时
//! 可能被按到，收一句「这一轮还在飞」比整个 wasm 实例 panic 掉强。
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
use crate::vision::KimiVisionConfig;
use crate::{config::HostConfig, db, events, history, session_id, tools, turn};

/// 页面手里的那个对象。
#[wasm_bindgen]
pub struct AgentHost {
    inner: Rc<Inner>,
}

struct Inner {
    config: HostConfig,
    /// 识图专用的 Kimi 连接配置，跟 `config` 那份主对话 provider 完全独立
    /// ——即使两者恰好都是 `"kimi"` 也不互相借用。`None` = 页面没配，
    /// `inspect_image()` 调用时才 reject（不是构造期硬错误）。见
    /// [`crate::vision`] 模块文档「key 从哪来」。
    vision: Option<KimiVisionConfig>,
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
        let vision = KimiVisionConfig::parse(config_json);
        Ok(AgentHost {
            inner: Rc::new(Inner {
                tool_table_json: tools::tool_table_json(&tools::browser_tool_table()),
                config,
                vision,
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

    /// 删掉一个会话：**journal 与图片一起没**。
    ///
    /// 删的是整个库（`agent-session-<id>`），所以图片不需要单独清——这正是 119
    /// §五-3 选「同一个库」换来的东西。schema 与连接管理的细节在 [`crate::db`]。
    ///
    /// 页面必须知道的三条：
    ///
    /// 1. **删当前打开的这个会话是允许的**，代价是它当场被关掉：`sessionId()`
    ///    变回 `undefined`，`send()` 会开始报「还没打开会话」，接下来开哪个由页面
    ///    决定。选「关掉」而不是「拒绝」的理由：这个宿主没有别的关会话的入口，
    ///    拒绝就等于「你正在看的这个会话永远删不掉」。
    /// 2. **页面自己那条 IndexedDB 连接要先 `db.close()`**（[`crate::db`] 模块文档
    ///    第 3 条）。没关的话这次调用 **reject**，不是挂住；错误的含义是「现在没
    ///    删成」，不是「什么都没发生」——见 [`crate::db::delete`]。
    /// 3. 这一轮对话还在飞的时候调它会 reject（`live` 正被 `send()` 借着）。
    ///    先 `cancel()`，等 `send()` 的 Promise settle 再删。
    ///
    /// 成功时 Promise 结果是 `undefined`。
    #[wasm_bindgen(js_name = deleteSession)]
    pub fn delete_session(&self, id: String) -> js_sys::Promise {
        let inner = Rc::clone(&self.inner);
        wasm_bindgen_futures::future_to_promise(async move {
            let id = session_id::validate(&id).map_err(js_error)?.to_string();
            // 先放掉 Rust 这边可能持有的那条连接，再去删。顺序反了就是自己把自己
            // 挡住（`onblocked`）。借用在这个块里开始、在这个块里结束——**不跨
            // `await`**，那正是模块文档那条借用纪律说的事。
            {
                let mut guard = inner.live.try_borrow_mut().map_err(|_| {
                    js_error("这一轮还在飞：先 cancel()，等 send() 的 Promise settle 再删")
                })?;
                if guard.as_ref().is_some_and(|live| live.id == id) {
                    *guard = None;
                    *inner.cancel.borrow_mut() = None;
                }
            }
            db::delete(&id).await.map_err(js_error)?;
            Ok(JsValue::UNDEFINED)
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
            // `Err` 是 M12 的 transient-source 出口。**124 之后它不再是不可达的**
            // ——工具表里有了 `web:source/` 前缀的工具（见 `crate::tools`），
            // 这条从没在浏览器里跑过的路第一次真的会亮。真出现就是一条给页面的
            // 错误，不是一个假的终态——`js_error` 会让那个 Promise reject。
            // （这里原本写着「这个宿主的工具表里结构上不可达」，那是 114 时的
            // 事实，124 推翻了它。）
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

    /// 把一张图交给识图服务（Kimi 3），拿回文字描述——119 §四那张分工表里
    /// 「Rust 那一格」的全部内容，接线细节见 [`crate::vision`] 模块文档。
    /// **这条不接工具、不接模型**：页面直接调，不经会话/工具执行路径，也
    /// 不需要先 `openSession`（见模块文档的借用纪律）。
    ///
    /// - `bytes` 上限是 [`crate::vision::MAX_BROWSER_IMAGE_BYTES`]
    ///   （**不是** `agent_transport::MAX_IMAGE_BYTES` 那个 Moonshot 100 MiB
    ///   传输上限——两者管的是不同的约束层，见 `vision.rs` 模块文档）；超限
    ///   直接 reject，不建 `Client`、不发任何网络请求。
    /// - Kimi 的 base_url/api_key 来自构造时配置 JSON 里一个独立的 `vision`
    ///   段，跟主对话 provider 无关——没配就 reject，措辞含
    ///   `not_configured`，对齐 `vision_inspect.rs` 同名错误，不 panic。
    /// - reject 的 message 里不含任何 key：[`crate::vision::inspect`] 自己
    ///   不拼 key 进消息，网络层错误则靠 125 的 redact。
    #[wasm_bindgen(js_name = inspectImage)]
    pub fn inspect_image(&self, bytes: Vec<u8>, mime: String, question: String) -> js_sys::Promise {
        let inner = Rc::clone(&self.inner);
        wasm_bindgen_futures::future_to_promise(async move {
            let text = crate::vision::inspect(inner.vision.as_ref(), bytes, mime, question)
                .await
                .map_err(js_error)?;
            Ok(JsValue::from_str(&text))
        })
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
