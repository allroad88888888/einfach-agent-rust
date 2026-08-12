//! **浏览器宿主**（issue 114c）：把同一套 `agent-runtime` 装配成一个跑在页面里、
//! 不需要任何服务端进程的 agent。决策 26 的第三种形态——独立跑 / 宿主子进程 /
//! **浏览器内**三者并存。
//!
//! # 这个 crate 只做「装配」，不做「实现」
//!
//! 事件泵、转移表、adapter、transport 一行都不在这里。浏览器与 CLI/server 的
//! 差别全部落在**三个外部输入**上，其余共用同一份代码：
//!
//! | 输入 | CLI | 本 crate |
//! |---|---|---|
//! | provider 配置与 key | `providers.toml` | 页面传进来（[`config`]，114d 的 `from_host`） |
//! | 会话落点 | `Jsonl`（文件） | IndexedDB（[`db`] 开库 + `agent-runtime` 的 `WebIdbStore`，114a/114c） |
//! | 工具 | `srv:` 本地表 + 真 executor | 两条 `web:` 声明 + `NullToolExecutor`（[`tools`]/[`host_tool`]，111 的裁剪 + 112 的接缝） |
//!
//! 平台差异**不在这个 crate 里**，也不在任何业务逻辑里：`fetch` vs. ureq 在
//! `agent-transport`（113），行源与心跳在 `agent-runtime` 的 `io_stream`/
//! `heartbeat` 两条接缝之下（114c）。红线 12 更强的那一面——`agent-core` 里没有
//! 任何 `#[cfg(target_arch)]`——因此是结构性成立的，不靠人盯。
//!
//! # 模块
//!
//! | | 那一件事 |
//! |---|---|
//! | [`host`] | 暴露给页面 JS 的类型 `AgentHost`（唯一的 `#[wasm_bindgen]` 面） |
//! | [`assemble`] | 一次「开会话」的装配线：开库 → store → 恢复 → `RunnerCtx` |
//! | [`turn`] | 一整轮对话：`begin_turn` 时机 + 宿主工具就地排空 + 取消轮丢弃 |
//! | [`config`] | 页面给的配置 JSON → `agent_transport::ProviderConfig` + adapter |
//! | [`tools`] | 浏览器形态的工具表（空表起步，只有 `web:`） |
//! | [`host_tool`] | 那两条 `web:` 工具真正读 `document.title` / `location.href` 的地方 |
//! | [`db`] | IndexedDB 库的 schema 与生死：一个会话一个库，`journal` + `images` 两张表，删会话 = 删整个库 |
//! | [`session_id`] | 会话 id 白名单（055 的规则，拒绝不 sanitize） |
//! | [`events`] | `AgentEvent` → 页面收得到的一条 JSON |
//! | [`history`] | 重放出来的会话历史 → 页面能重画的一份 JSON |
//!
//! # key
//!
//! 111 契约第 4 条：**每个用户一把自己的 key**。这个 crate 里没有任何默认 key，
//! 没有任何路径把 key 打印/序列化出去（`ProviderConfig` 的 `Debug` 手写只吐长度，
//! [`config::HostConfig`] 干脆不派生 `Debug`），配套的 `www/index.html` 也不内置
//! 任何 key——它只有一个输入框。

mod assemble;
mod config;
mod db;
mod events;
mod history;
mod host;
mod host_tool;
mod session_id;
mod tools;
mod turn;

pub use host::AgentHost;
