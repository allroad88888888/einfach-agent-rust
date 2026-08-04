//! 宿主注入的工具怎么进**这个会话**的表（062，接缝见 `docs/HOST-CAPABILITIES.md`
//! §二/§五/§六）。
//!
//! 从 `tool_table.rs` 分出来的一件事：`POST /sessions` 那一次声明进来的工具
//! （`web:`/`desk:`）怎么追加进表尾、可逆性记在哪儿。工具表的五档装配、名字规则、
//! `snapshot` 的另外两级留在 [`super`]。
//!
//! # 三条硬规矩
//!
//! 1. **per-session**（§二，062 最重要的一条）：这张映射是 `ToolTable` 的一部分，
//!    而 `ToolTable` 是每个会话在自己的 actor 线程里现造的一份
//!    （`agent_server::actor::body` 拿 `OpenSpec` 建）。注入的工具因此进不了别的
//!    会话、也进不了任何全局表——**这里没有给全局表开写口**。
//! 2. **追加在表尾**（红线 11，§六 第 1 条）：连 MCP 之后。前面那一段所有会话共有的
//!    字节一个都不动，前缀缓存不因为某个客户端声明了两个工具就整体作废。
//! 3. **进表前按名字排序**（§六 第 2 条）：客户端给的数组顺序**不可靠**（同一份声明
//!    两次连接可能不同序），而它会变成 prompt 字节。排序放在这里而不是放在 HTTP
//!    那一层——它是「进 prompt 的东西必须逐字节确定」这条红线的落点，谁调
//!    [`ToolTable::with_host_tools`] 都该白拿到，不能靠调用方记得先排。
//!
//! # 可逆性为什么不进 `ToolSpec`
//!
//! `ToolSpec` 的三个字段**进 prompt**，加字段要重算红线 11 的账；可逆性纯查表、
//! 不进 prompt。所以照 `mcp_reversibility` 的既有先例另挂一张有序表
//! （HOST-CAPABILITIES §五），`ToolSpec` 的形状一个字节不动。
//!
//! # 查表的门为什么按「表」而不按「前缀」
//!
//! [`ToolTable::snapshot`] 里三级优先级的第一级是「注入映射里有没有这个名字」，
//! 不是「名字是不是 `web:`/`desk:` 开头」。062 之前那一段的形状是
//! `if tool.starts_with("mcp:")`——照抄它给注入表配一个 `web:`/`desk:` 前缀门，
//! 表面上等价，实际上会让**任何没被注入的** `web:` 工具（比如 `standard` 档自带的
//! `browser_action`）也走进注入分支查一次空表，更要命的是反过来：真正注入的工具
//! 一旦哪天前缀白名单变了，就静默落回 `reversibility_of` 的 `_ => Irreversible`
//! 兜底——宿主声明了 `pure` 却按 `Irreversible` 办，**功能「正常」、一声不吭**，
//! 只有 `/undo` 撞上它停下来问的时候才看得出来。按表查没有这个耦合：这张表里有
//! 谁，谁就按声明办，跟名字长什么样无关。

use std::sync::Arc;

use agent_core::{Reversibility, ToolSpec};

use super::ToolTable;

impl ToolTable {
    /// 062 装载：把宿主这一次建会话时声明的工具追加进**这个会话**的表，并把每个
    /// 工具的可逆性记进注入映射——[`ToolTable::snapshot`] 从此**最先**查这份映射
    /// （三级优先级写死在那里）。
    ///
    /// 入参就是 `agent_server` 的 `capabilities.tools` 翻译产出的
    /// `(ToolSpec, Reversibility)`，跟 [`ToolTable::with_mcp`] 同一个形状（041/043
    /// 的既有先例）。**宿主没声明可逆性的那些，调用方在翻译时就该落成保守的
    /// `Irreversible`**（HOST-CAPABILITIES §五：「没说」不能推定为「安全」）——这里
    /// 不替它猜，收到什么记什么。
    ///
    /// 排序、追加位置的理由见本模块文档。链式调用该排在 `with_skills`/`with_mcp`
    /// **之后**（宿主注入是表尾那一段）。
    ///
    /// **撞名**（075）：客户端请求体里有两个同名声明，或者跟表里已有的名字撞了——
    /// 走 [`ToolTable::push_spec`]，后来的那一条整条丢弃（连可逆性也不 `insert`），
    /// 不 panic。这是运行时数据（客户端请求体），069 §拍板 D 否决过让它硬失败。
    pub fn with_host_tools(mut self, mut tools: Vec<(ToolSpec, Reversibility)>) -> Self {
        tools.sort_by(|(a, _), (b, _)| a.name.cmp(&b.name));
        for (spec, reversibility) in tools {
            let name = Arc::clone(&spec.name);
            if self.push_spec(spec) {
                self.host_reversibility.insert(name, reversibility);
            }
        }
        self
    }
}

#[cfg(test)]
#[path = "tool_table_host_tests.rs"]
mod tests;
