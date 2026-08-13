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
//! | 工具 | `srv:` 本地表 + 真 executor | 三条内建 `web:` 声明 + **页面自己声明的那一段** + 页面装的执行回调 + `NullToolExecutor`（[`tools`]/[`host_tool`]/[`callback`]，111 的裁剪 + 112 的接缝 + 121 的回调 + 122 的声明入口） |
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
//! | [`host`] | `AgentHost` 上**不碰 `live`** 的那一面：构造、装回调、工具表/key 长度/识图 |
//! | [`host_session`] | `AgentHost` 上**碰 `live`** 的那一面：开/切/删会话、说一句话、取消、查历史。「借用纪律」住在这里 |
//! | [`callback`] | 页面装进来的 JS 函数：事件 sink、store 错误 sink、工具执行回调（121） |
//! | [`assemble`] | 一次「开会话」的装配线：开库 → store → 恢复 → `RunnerCtx` |
//! | [`turn`] | 一整轮对话：`begin_turn` 时机 + 宿主工具就地排空 + 取消轮丢弃 |
//! | [`interrupt`] | 工具执行期间的两条打断路：用户取消 / 截止线到点（123） |
//! | [`config`] | 页面**建宿主那一刻**给定、此后不变的外部输入：provider 配置 + 声明好的工具那一段 |
//! | [`tools`] | 浏览器这张工具表怎么装出来（空表起步 + 三条内建 + 直接工具 + skill read）。`capabilities` 的 JSON 入口在 [`capabilities`]；顶层工具继续复用 runtime 校验 |
//! | [`host_tool`] | `web:` 工具的执行与派发顺序：内建（读 `document.title` / `location.href` / 回显）优先，没命中才交给页面回调 |
//! | [`db`] | IndexedDB 库的 schema 与生死：一个会话一个库，`journal` + `images` 两张表，删会话 = 删整个库 |
//! | [`session_id`] | 会话 id 白名单（055 的规则，拒绝不 sanitize） |
//! | [`events`] | `AgentEvent` → 页面收得到的一条 JSON |
//! | [`history`] | 重放出来的会话历史 → 页面能重画的一份 JSON |
//! | [`vision`] | `inspectImage` 的四步编排 + 独立于主 provider 的 Kimi 连接配置（127） |
//!
//! # key
//!
//! 111 契约第 4 条：**每个用户一把自己的 key**。这个 crate 里没有任何默认 key，
//! 没有任何路径把 key 打印/序列化出去（`ProviderConfig` 的 `Debug` 手写只吐长度，
//! [`config::HostConfig`] 干脆不派生 `Debug`），配套的 `www/index.html` 也不内置
//! 任何 key——它只有一个输入框。

mod assemble;
mod callback;
mod capabilities;
mod config;
mod db;
mod events;
mod history;
mod host;
mod host_session;
mod host_tool;
mod interrupt;
mod session_id;
mod tools;
mod turn;
mod undo;
mod vision;

pub use host::AgentHost;
