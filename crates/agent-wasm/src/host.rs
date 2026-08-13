//! [`AgentHost`]：页面手里的那个对象——**怎么造它、怎么给它装回调、怎么问它那些
//! 不依赖任何已打开会话的东西**（工具表、key 长度、识图）。
//!
//! # 这个文件与 [`crate::host_session`] 的界线：碰不碰 `live`
//!
//! 开会话、说一句话、取消、切会话、删会话、查历史——**凡是碰 `live`
//! （`Session` + `RunnerCtx`）的入口全部在 [`crate::host_session`]**。这条界线不是
//! 随手划的：它正好是「借用纪律」管辖的边界（`send()` 在整轮对话期间持有
//! `live.borrow_mut()`），纪律跟它管的代码住在一起才不会各说各话。
//!
//! 留在这个文件里的五个入口——[`AgentHost::on_event`]、[`AgentHost::on_tool_call`]、
//! [`AgentHost::tool_table_json`]、[`AgentHost::key_len`]、[`AgentHost::inspect_image`]
//! ——一个都不碰 `live`，所以**在一轮对话飞着的时候调它们是安全的**，包括在工具
//! 回调里面调。识图尤其：它不依赖任何已开的会话，页面不用先 `openSession` 就能调
//! （见 [`crate::vision`] 模块文档）。
//!
//! # 页面装的回调都挂在这里，不挂在 `RunnerCtx` 上
//!
//! 切会话时 `RunnerCtx` 整个换掉，页面装一次的回调不该跟着掉。存放形状与「工具
//! 回调怎么被 [`crate::host_tool`] 取到」见 [`crate::callback`] 模块文档。
//!
//! # key 只从使用者来
//!
//! 构造这个类型的唯一入口收一份页面给的配置 JSON（[`crate::config`]），**代码里
//! 没有任何默认 key，也没有任何地方把 key 打印出来**（111 契约第 4 条）。

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use wasm_bindgen::prelude::*;

use crate::assemble::Live;
use crate::callback::{self, Slot};
use crate::capabilities;
use crate::config::HostConfig;
use crate::tools;
use crate::vision::KimiVisionConfig;

/// 页面手里的那个对象。
#[wasm_bindgen]
pub struct AgentHost {
    pub(crate) inner: Rc<Inner>,
}

pub(crate) struct Inner {
    pub(crate) config: HostConfig,
    /// 识图专用的 Kimi 连接配置，跟 `config` 那份主对话 provider 完全独立
    /// ——即使两者恰好都是 `"kimi"` 也不互相借用。`None` = 页面没配，
    /// `inspect_image()` 调用时才 reject（不是构造期硬错误）。见
    /// [`crate::vision`] 模块文档「key 从哪来」。
    pub(crate) vision: Option<KimiVisionConfig>,
    /// 建好就不变——`tool_table_json()` 因此不需要借 `live`（见模块文档那条界线），
    /// 页面在任何时刻都能取到它做字节比对。
    pub(crate) tool_table_json: String,
    /// 页面装的事件回调。见 [`crate::callback`] 模块文档：为什么是
    /// `Rc<RefCell<_>>`，为什么不进 `RunnerCtx`。
    pub(crate) on_event: Slot,
    /// 页面装的**工具执行回调**（121）。跟 `on_event` 同款同理由；额外的一条是
    /// 它必须被 [`crate::host_tool`] 那条 await 链够到，而那条链上没有
    /// `AgentHost`——桥在 [`crate::callback`]。
    pub(crate) on_tool_call: Slot,
    pub(crate) live: RefCell<Option<Live>>,
    /// 当前会话的取消标志。它必须能在 `live` 被借着的时候被翻——见
    /// [`crate::host_session`] 模块文档。
    pub(crate) cancel: RefCell<Option<Arc<AtomicBool>>>,
}

#[wasm_bindgen]
impl AgentHost {
    /// 收两份页面给的东西：
    ///
    /// 1. `config_json`：
    ///    `{"provider":"deepseek|kimi|glm","base_url":"…","model":"…","api_key":"…"}`
    ///    （识图另有一个可选的 `vision` 段，见 [`AgentHost::inspect_image`]）；
    /// 2. `capabilities_json`（**可选**）：这个页面自己实现、要交给模型用的能力表。
    ///    旧的只带工具形状仍兼容：
    ///    ```json
    ///    {"tools":[{"name":"web:crm/lookup","description":"按客户 ID 查 CRM 档案。无参数。",
    ///               "schema":{"type":"object","properties":{},"additionalProperties":false},
    ///               "reversibility":"pure"}]}
    ///    ```
    ///    也可追加 skill：`{"skills":[{"id":"crm-flow","description":"…","body":"…"}]}`。
    ///    skill 正文由 `srv:skill/read` 按需读，开局只写入索引；v1 不接受 skill
    ///    自带 `tools`。直接 `web:`/`desk:` 工具仍由 [`AgentHost::on_tool_call`] 的
    ///    回调执行。不给（或给 `null`）= 只有三条内建工具。
    ///    还可声明开局块（决策 31，157）：
    ///    `{"prefix":[{"name":"web:ops/briefing","text":"今天的上下文……"}]}`——
    ///    页面自己跑完逻辑把**结果文本**带进来，新会话开局落成 system 前缀块；
    ///    名字规则与 `tools` 一字不差（`web:`/`desk:` + 同款白名单），`text` 不能空。
    ///
    /// # 声明的规则
    ///
    /// - `name` **必须**以 `web:` 或 `desk:` 开头，前缀之后只许 ASCII 字母/数字/
    ///   `-`/`_`/`/`，全名 ≤128 字节。`srv:`/`mcp:` 是服务端执行的前缀，**当场拒**。
    /// - `description` 与 `schema` **原样进 prompt**，一个字节都不会被改写
    ///   （不 trim、不补标点、不重排）。`schema` 缺省 `{"type":"object"}`。
    /// - `reversibility` 是小写三档 `pure|reversible|irreversible`；**不写就按
    ///   `irreversible` 办**——「没说」不能推定为「安全」，`/undo` 撞上它会停下来问。
    /// - 名字跟内建的 `web:page/title`、`web:page/url`、`web:source/echo` 撞了 → 拒。
    ///   那三条由 Rust 执行，声明和实现必须同源。
    /// - **一条不合法 = 整份拒**，这个构造函数当场 `throw` 并说清是哪一项。不会出现
    ///   「悄悄少了一条」——那正是最难查的一种。
    ///
    /// # ⚠️ 每次刷新给的这份 JSON 必须逐字节一样
    ///
    /// 工具表进 prompt **最前面**，前缀缓存靠逐字节相等命中。运行时只帮你两件事：
    /// 数组顺序会按名字排掉，`schema` 里的键序会归一。**帮不了的是字段顺序和描述
    /// 文案**——描述改一个字、schema 多一个键、少一条工具，刷新之后前缀缓存全断
    /// （DeepSeek 上是 120 倍的差价，而且功能完全正常，没有任何报错）。
    ///
    /// 所以：**把它写成一个模块级常量字符串，不要每次现拼**。新会话会把解析后的
    /// 直接工具与 skill 写进自己的 journal；恢复时只重放 journal 中那一份，不用当前
    /// 宿主配置覆盖历史。
    ///
    /// ```js
    /// const PAGE_TOOL_DECLARATION = '{"tools":[{"name":"web:host/callback-probe", … }]}';
    /// const host = new AgentHost(configJson, PAGE_TOOL_DECLARATION);
    /// ```
    ///
    /// 拿 [`AgentHost::tool_table_json`] 在刷新前后各取一次做字节比对，就能自己验。
    ///
    /// # 新会话的表在这一刻定死
    ///
    /// 没有「中途改表」的入口，这是**故意的**：会话中途换表就是前缀缓存全断，而且
    /// 已经发生过的那几轮对话是在旧表下答的。想换表 = 重新 `new AgentHost(...)`，
    /// 那本来就是一个全新的宿主。已存在会话是唯一的例外：它只重放自己的 journal，
    /// 不受新宿主的配置影响。
    #[wasm_bindgen(constructor)]
    pub fn new(config_json: &str, capabilities_json: Option<String>) -> Result<AgentHost, JsValue> {
        let config = HostConfig::parse(config_json).map_err(js_error)?;
        // 名字不认识就当场报，不要等第一次请求才发现 endpoint 和编码对不上。
        config.adapter().map_err(js_error)?;
        // 同理：声明写错了当场报，不要等模型第一次调它才发现表里没有。
        let declared = capabilities::parse(capabilities_json.as_deref()).map_err(js_error)?;
        let config =
            config.with_declared_capabilities(declared.tools, declared.skills, declared.prefix);
        let vision = KimiVisionConfig::parse(config_json);
        Ok(AgentHost {
            inner: Rc::new(Inner {
                tool_table_json: tools::tool_table_json(&tools::browser_tool_table(
                    config.declared_tools(),
                    config.declared_skills().to_vec(),
                    config.declared_prefix(),
                )),
                config,
                vision,
                on_event: Rc::new(RefCell::new(None)),
                on_tool_call: Rc::new(RefCell::new(None)),
                live: RefCell::new(None),
                cancel: RefCell::new(None),
            }),
        })
    }

    /// 装一条事件回调：`handler(jsonString)`。形状见 `events.rs`。
    ///
    /// 跟工具回调受同一条重入约束（见 [`AgentHost::on_tool_call`] 那张表），
    /// 但事件回调本来就只该**读、画**，不该在里面干活。
    #[wasm_bindgen(js_name = onEvent)]
    pub fn on_event(&self, handler: js_sys::Function) {
        *self.inner.on_event.borrow_mut() = Some(handler);
    }

    /// 装一条**工具执行回调**：`handler(name, inputJson) -> Promise<string>`。
    ///
    /// 模型调了一条这个宿主**内建不认识**的 `web:` 工具时，交给它执行，`await`
    /// 它返回的 Promise。那些工具正是构造时用 `capabilities_json` 声明进来的
    /// 那一段（122）——**声明是它们能被调到的前提**：没声明的名字连等待槽都不会
    /// 开，模型编一个也调不到这里。
    ///
    /// - resolve 出来的字符串 = 工具结果正文；
    /// - **resolve 出来不是字符串** → 一条 `is_error` 的结果，不 panic；
    /// - **reject 或同步 `throw`** → 一条 `is_error` 的结果，正文取异常的 `message`；
    /// - **没装这条回调**而模型调了内建之外的工具 → 同样是一条 `is_error` 的结果，
    ///   措辞是 [`crate::host_tool`] 那句「这个宿主没有实现工具 `…`」。
    ///
    /// 四条全是「模型收到 `is_error` 自己纠」，**没有一条会挂死或崩页面**。
    ///
    /// # 内建优先
    ///
    /// `web:page/title`、`web:page/url`、`web:source/echo` 永远由 Rust 执行，装了
    /// 回调也一样。页面**不能**悄悄劫持一条已声明工具的语义——那会让
    /// [`AgentHost::tool_table_json`] 描述的东西和真正执行的东西对不上，**且不报错**。
    ///
    /// 122 之后这一条有了第二道闸：这三个名字**连声明都不许**（在
    /// [`AgentHost::new`] 就被拒），所以「用同名覆盖掉内建的描述」也做不到。
    ///
    /// # ⚠️ 回调里不要回头调这个 `AgentHost`
    ///
    /// 这条回调是在 `send()` 持有 `live` 可变借用**之内**被调用的
    /// （[`crate::host_session`] 模块文档「借用纪律」）。于是：
    ///
    /// | 在回调里调 | 会发生什么 |
    /// |---|---|
    /// | `send()` / `openSession()` / `sessionId()` / `historyJson()` | **panic**（`already borrowed`），panic 会带走整个 wasm 实例 |
    /// | `deleteSession()` | reject（它用 `try_borrow_mut`），不 panic，但在这一轮里它**永远**删不成 |
    /// | `cancel()` / `toolTableJson()` / `keyLen()` / `inspectImage()` | 安全：这四个不碰 `live` |
    ///
    /// **工具回调天然要干活**——它就是来干活的，这没问题：`fetch`、IndexedDB、DOM、
    /// `await` 任意 Promise、以及 `inspectImage()`（浏览器识图那条路正是这么走的）
    /// 全都随便用。不能干的只有一件事：**回头经过这个 `AgentHost` 去碰会话**。
    ///
    /// 为什么不学 [`AgentHost::delete_session`] 用 `try_borrow_mut` 把 panic 变成
    /// reject：那条是**从借用外面**打进来的调用，「撞上在飞的一轮」是两个独立操作
    /// 的竞争，reject 的含义「现在没删成，等这轮完再来」是**真话**。回调里的
    /// `send()` 不是竞争——它嵌套在自己那一轮里，等多久都不会成立。reject 会把一个
    /// 结构性错误说成一次可重试的碰撞，页面照着 retry 就是死循环。宁可当场炸。
    #[wasm_bindgen(js_name = onToolCall)]
    pub fn on_tool_call(&self, handler: js_sys::Function) {
        callback::install_tool(&self.inner.on_tool_call, handler);
    }

    /// 这个宿主给新会话的工具表，原样序列化。它是刷新前后逐字节比对，以及检查
    /// skill 仅追加 `srv:skill/read`（不含 MCP）的证据面，见 [`crate::tools`] 模块文档。
    #[wasm_bindgen(js_name = toolTableJson)]
    pub fn tool_table_json(&self) -> String {
        self.inner.tool_table_json.clone()
    }

    /// key 的**长度**，不是 key。页面横幅只许显示这个。
    #[wasm_bindgen(js_name = keyLen)]
    pub fn key_len(&self) -> usize {
        self.inner.config.key_len()
    }

    /// 把一张图交给识图服务（Kimi 3），拿回文字描述——119 §四那张分工表里
    /// 「Rust 那一格」的全部内容，接线细节见 [`crate::vision`] 模块文档。
    /// **这条不接工具、不接模型**：页面直接调，不经会话/工具执行路径，也
    /// 不需要先 `openSession`（见模块文档那条界线）。页面在工具回调里调它是
    /// **正常用法**，不是钻空子——它不碰 `live`。
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

pub(crate) fn js_error(message: impl AsRef<str>) -> JsValue {
    js_sys::Error::new(message.as_ref()).into()
}
