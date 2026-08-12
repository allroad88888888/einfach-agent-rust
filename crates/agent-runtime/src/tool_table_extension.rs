//! 一个 [`ExtensionPack`] 怎么装进一个会话（148，接缝文档
//! [`docs/EXTENSIONS.md`](../../../docs/EXTENSIONS.md) §两阶段装配）。
//!
//! 从 `tool_table.rs` 分出来的一件事：**装配这件事**。包本身长什么样在
//! [`crate::extension_pack`]，表的五档装配/名字规则/`snapshot` 的三级优先级留在
//! [`super`]。
//!
//! # 装配天然是两阶段，接缝必须承认它
//!
//! 一个包里的两类东西住在两个**不同时刻才存在**的容器里：
//!
//! | 半边 | 装进哪儿 | 那一刻宿主手上有什么 |
//! |---|---|---|
//! | specs + 可逆性 + timed | [`ToolTable`] | 只有表——`RunnerCtx::new` 还没调，ctx 不存在 |
//! | 截获执行体 | [`RunnerCtx`] | ctx 已经建好，而它**吃掉了**那张表 |
//!
//! 这不是接缝设计得不好，是既有结构的事实：`RunnerCtx::new` 按值收 `ToolTable`
//! （表是会话的一部分，不是共享物），而 146 的截获注册表住在 `RunnerCtx` 上
//! （执行体跟 executor 一样是 ctx 的资源；timed 执行体在表里是那条注释解释过的
//! 例外——`run_session_start` 跑在 ctx 建成之前，手上只有表）。所以不存在
//! 「一次调用吃一包」的写法，只有「一包拆两次装」。
//!
//! 于是接缝定成：[`ToolTable::with_extension`] 装表半边、**并把 ctx 半边打包成一个
//! 必须被消费的中间产物** [`PendingInterceptors`] 交出来。
//!
//! ```ignore
//! let (tools, pending) = ToolTable::builtin().with_shell().with_extension(demo_pack());
//! let mut ctx = RunnerCtx::new(/* … */, tools, /* … */);
//! pending.install(&mut ctx);
//! ```
//!
//! # 防呆：「装了表半边、忘了 ctx 半边」在这里装不出来
//!
//! 半开的后果是静默的：specs 进了 prompt，模型看得见这个工具、调它，dispatch 查
//! 截获表查不到 → 落进常规 `ExecuteTool` 路 → `unknown_tool`。**不报错，只是这个
//! 扩展永远不工作**，而故障现象（模型抱怨工具不存在）离肇事的那一行隔着整条链路。
//!
//! 三道锁，从「编译器能说的」到「一定会说的」：
//!
//! 1. **[`PendingInterceptors`] 不是 `Clone`、也没有公开构造器**——它只能由
//!    [`ToolTable::with_extension`] 从**同一个包实例**拆出来的执行体造出来
//!    （[`ExtensionPack::into_parts`](crate::ExtensionPack) 消费自身）。
//!    「表装 A 包、ctx 装 B 包」因此不是「要小心的事」，是**写不出来的事**。
//! 2. **`#[must_use]`**：`with_extension(..)` 的返回值被整个丢掉时编译器就警告。
//! 3. **析构炸弹**：绑给了 `_pending` 却从没 `install` 的那一半——`Drop` 里
//!    `debug_assert!` 当场炸（点名是哪个包），release 落一条 `tracing::error!`。
//!    正在 unwind 时不炸（`thread::panicking()` 先挡）：drop 里再 panic 会直接
//!    `abort`，把一次本来看得见的失败换成一个没有栈的进程死亡。
//!
//! 为什么 Rust 里做不到「不装就不编译」：语言没有线性类型，`Drop` 是「值一定会
//! 被处理」这句话唯一的落点。所以这里选的是**一定会说话的运行期锁**，而不是自我
//! 感觉良好的编译期锁。
//!
//! **空包也必须装**：一个只带 timed 钩子的包拆出来的 [`PendingInterceptors`] 是
//! 空的，丢了也无害——但它照样炸。理由是纪律要对宿主统一：宿主不知道（也不该知道）
//! 某个包里有没有截获工具，今天空、下个版本加了一条截获的包，会让一个「反正是空的
//! 就没写 install」的宿主**在升级依赖那一刻静默半开**。这正是本节要挡的那件事。
//!
//! # 可逆性复用 `host_reversibility`，不新开第三张表
//!
//! 扩展工具的可逆性走**跟 M10 注入工具同一张映射**（[`ToolTable::snapshot`] 三级
//! 优先级的第一级，`tool_table_host.rs` 那张）。理由三条：
//!
//! 1. **语义逐字相同**：那一级答的是「有人在装配期按名字**显式声明**过这个工具的
//!    可逆性吗」——包作者填的第二个位置参数正是这句话。差别只在声明**来源**
//!    （编译期 Rust 依赖 vs 一次 HTTP 请求体），而来源今天**没有读者**：
//!    `snapshot` 只问值是多少。为一个没人读的维度新开一张表，就是 `docs/TOOLS.md`
//!    §「没有 `Source` 枚举」拒绝过的第二份真相。
//! 2. **那张表的门是按表查、不按前缀查**（062 特意选的，见 `tool_table_host.rs`
//!    §「查表的门为什么按表而不按前缀」）——它对 `ext:` 这个前缀天然成立，一个
//!    字节都不用改；新开一张表反而要在 `snapshot` 里加第四个分支，而那个分支的
//!    行为会跟第一级逐字一样。
//! 3. **撞不了键**：注入名强制 `web:`/`desk:`、扩展名强制 `ext:`，069 的命名红利
//!    保证两族结构上不相交；即便有人绕过前缀闸，[`ToolTable::push_spec`] 也会先
//!    在 specs 区拦下后来的那一条，映射根本走不到 `insert`。
//!
//! # 顺序：包内顺序原样保留，**不排序**
//!
//! `with_host_tools` 进表前按名字排序，因为客户端给的数组顺序不可靠（同一份声明
//! 两次连接可能不同序，而它会变成 prompt 字节）。包不是这样：条目顺序是**源码里
//! 写死的**，`Vec` 的 push 顺序即字节顺序，本来就逐字节确定（红线 11）。多排一次
//! 只会让作者看到的顺序和模型看到的顺序对不上。
//!
//! 追加位置照旧是**表尾**：前面那一段所有会话共有的字节一个都不动。

use std::sync::Arc;

use crate::SessionToolFn;
use crate::ctx::RunnerCtx;
use crate::extension_pack::ExtensionPack;

use super::ToolTable;

impl ToolTable {
    /// 148：把一个扩展包的**表半边**装进这张表——specs 走
    /// [`ToolTable::push_spec`] 判重、可逆性进注入映射、timed 条目走
    /// [`ToolTable::with_timed`]——并把 **ctx 半边**打包成 [`PendingInterceptors`]
    /// 交回给调用方。
    ///
    /// 两半边必须来自同一个包实例，机制见模块文档「防呆」第 1 条。
    ///
    /// **撞名**：specs 撞了（表里已有同名，或包内自己重复）→ `push_spec` 那条既有
    /// 判据，`debug_assert!` + release 整条丢弃；被丢的那条**执行体也不会进**
    /// `PendingInterceptors`（这里靠 `push_spec` 的返回值决定要不要往下带，同
    /// `with_mcp`/`with_host_tools` 处理可逆性映射的写法）——声明与执行路径同进
    /// 同出这句话，在「被丢弃」这条路上同样成立，否则 `install` 会拿一个表里根本
    /// 没有的名字去撞 146 的第二道闸，一个问题炸两次。
    pub fn with_extension(mut self, pack: ExtensionPack) -> (ToolTable, PendingInterceptors) {
        let (pack_name, tools, timed) = pack.into_parts();

        let mut pending = Vec::with_capacity(tools.len());
        for (spec, reversibility, run) in tools {
            let name = Arc::clone(&spec.name);
            if self.push_spec(spec) {
                self.host_reversibility
                    .insert(Arc::clone(&name), reversibility);
                pending.push((name, run));
            }
        }

        for (spec, timing, run) in timed {
            self = self.with_timed(spec, timing, run);
        }

        (
            self,
            PendingInterceptors {
                pack: pack_name,
                tools: pending,
                installed: false,
            },
        )
    }
}

/// 一个包**还没装的 ctx 半边**：那些截获执行体，等一个 [`RunnerCtx`]。
///
/// 不是 `Clone`、没有公开构造器、必须被 [`PendingInterceptors::install`] 消费
/// ——三道锁与理由全在模块文档「防呆」。**空的也必须装**。
#[must_use = "扩展包的 ctx 半边还没装：丢掉它 = specs 进了 prompt 但截获从未注册，\
              模型调这个工具只会拿到 unknown_tool。调 PendingInterceptors::install(&mut ctx)"]
pub struct PendingInterceptors {
    pack: Arc<str>,
    tools: Vec<(Arc<str>, SessionToolFn)>,
    installed: bool,
}

impl PendingInterceptors {
    /// 装 ctx 半边：逐条 `RunnerCtx::register_session_tool`（146 的三道闸在那边，
    /// 这里不重复判）。
    ///
    /// `ctx` 必须是**吃了配套那张表**的那个 ctx。装错了不会静默：146 的第二道闸
    /// 要求注册名已经在 `declares()` 里，而它只可能出自 [`ToolTable::with_extension`]
    /// 装过的那张表——所以「表装进了 A 会话、执行体装给了 B 会话」会在这里
    /// `debug_assert!` 炸出来，不会变成 B 会话里一个没人声明却能被执行的名字。
    pub fn install(mut self, ctx: &mut RunnerCtx) {
        for (name, run) in std::mem::take(&mut self.tools) {
            ctx.register_session_tool(name, run);
        }
        self.installed = true;
    }
}

impl Drop for PendingInterceptors {
    fn drop(&mut self) {
        if self.installed || std::thread::panicking() {
            return;
        }
        let pack = &self.pack;
        tracing::error!(
            pack = %pack,
            "扩展包只装了表半边：specs 已经进 prompt，截获执行体从未注册——\
             这个包的工具会被模型调到，然后落 unknown_tool"
        );
        debug_assert!(
            false,
            "扩展包 `{pack}` 的 PendingInterceptors 没被 install 就丢了：\
             表半边（specs/可逆性/timed）已经装进 ToolTable，ctx 半边（截获执行体）\
             永远不会注册。装配处补一行 pending.install(&mut ctx)"
        );
    }
}

#[cfg(test)]
#[path = "tool_table_extension_fixtures.rs"]
mod fixtures;

#[cfg(test)]
#[path = "tool_table_extension_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "tool_table_extension_guard_tests.rs"]
mod guard_tests;
