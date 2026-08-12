//! 宿主声明的开局块怎么进这张表（决策 31，M17，接缝见
//! `docs/HOST-CAPABILITIES.md` §八之三、issue 155）。
//!
//! 从 `tool_table_host.rs` 分出来的姊妹事：那份注入的是**工具**（模型面
//! `ToolSpec`），这份注入的是**一段常量文本**——宿主建会话前自己跑完逻辑，把
//! 结果文本直接带进来，装配期原样合成一条「执行体 = 返回这段文本」的
//! `SessionStart` timed 工具，追加进这张表。
//!
//! # 为什么合成 timed 条目，不是另开一条「宿主前缀」通道
//!
//! 前缀块今天只有一个生产者——`run_session_start` 消费 `timed(SessionStart)`
//! 落块（135）。宿主声明只要变成一批普通 timed 条目，就不需要
//! `run_session_start`/恢复/`spawn` 的 `inherit_prefix` 校验（`check_prefix_allowed`
//! 读的正是 timed 区 spec 名）为「第二个来源」各多写一次判断——三处机制零改动
//! 认得它，这正是决策 31 那张表的第一行，也是这个合成存在的全部理由。
//!
//! # 表尾 + 内部排序，两道闸独立成立
//!
//! 调用点（154/156 落地时）约定 `with_host_prefix` 排在装配链**最后**，在
//! `with_host_tools` 之后：内置 timed（比如 skills 索引）先注册，声明块因此
//! 永远排在最后，所有会话共有的那段字节不因为宿主声明了几个开局块而挪位——
//! 这是**装配顺序**给的第一道闸。这里再按 name 排序一次是第二道：防调用方没走
//! `Slot::HostPrefix` 那条正门（154），直接拿一份未排序的 `pairs` 喂进来。两道
//! 闸各自成立、互不依赖——红线 11 管的是「会进 prompt 的字节确不确定」，不能
//! 只靠调用方自觉排过一次。
//!
//! # spec 的 `description`/`schema` 是兜底值，`text` 才是唯一进 prompt 的字段
//!
//! timed 工具永不进模型面（`specs()`/`declares()` 看不见它，见
//! `tool_table_timed` 模块文档「timed 工具住独立区」），合成出来的 `ToolSpec`
//! 只给驱动/调试读 name，`description`/`schema` 没有任何观众，因此是占位值，
//! 不必也不该来自声明——声明里唯一落进前缀块（进而落进 prompt）的字段就是
//! `text` 本身，原样搬进执行体的返回值，一个字节不加工。

use std::sync::Arc;

use agent_core::ToolSpec;

use super::{CallTiming, ToolTable};

/// 宿主声明开局块用的兜底 spec：`name` 是声明给的规范名（前缀/重名校验是 156
/// 那层 HTTP 校验的事，这里不重复判断），`description`/`schema` 是占位值——
/// 理由见模块文档最后一节。
fn host_prefix_spec(name: Arc<str>) -> ToolSpec {
    ToolSpec {
        name,
        description: Arc::from("宿主声明的开局块（决策 31），不进模型面。"),
        schema: Arc::new(serde_json::json!({ "type": "object" })),
    }
}

impl ToolTable {
    /// 155：把宿主声明的 `(name, text)` 对合成为一批 `SessionStart` timed 工具，
    /// 追加进这张表。每对的执行体是 `move |_, _, _| Ok(text.clone())`——本地
    /// 同步、不读表内数据、不读会话状态，`text` 就是它的全部答案。
    ///
    /// **按 name 排序后注册**（模块文档「表尾 + 内部排序」）：`pairs` 的输入序
    /// 不保证确定，注册序才是前缀块序（`run_session_start` 按 timed 区的注册序
    /// 落块，135 的既有语义一行不动）。
    ///
    /// **空切片是彻底的无操作**：不排序、不分配、直接把 `self` 原样交回——跟
    /// 压根没调过这个方法逐字节相同（不带 `capabilities.prefix` 的会话必须
    /// 走这条路，跟 `with_host_tools`/`with_skills` 对空输入的既有取向一致）。
    ///
    /// **撞名不在这里判断**：声明内部重名、或撞了已经注册的 specs/timed 名字，
    /// 全部由 [`ToolTable::with_timed`] 的既有双向查兜底（整条丢弃 + debug
    /// 断言）。156 的 HTTP 校验会在更早的一层把「声明内部重名」结构化拒掉并
    /// 点名，这里的兜底只服务「调用方没走正门」的防御性场景。
    pub fn with_host_prefix(mut self, pairs: &[(Arc<str>, Arc<str>)]) -> Self {
        if pairs.is_empty() {
            return self;
        }
        let mut sorted: Vec<&(Arc<str>, Arc<str>)> = pairs.iter().collect();
        sorted.sort_by(|(a, _), (b, _)| a.cmp(b));
        for (name, text) in sorted {
            let name = Arc::clone(name);
            let text = Arc::clone(text);
            self = self.with_timed(
                host_prefix_spec(name),
                CallTiming::SessionStart,
                Box::new(move |_table, _session, _input| Ok(Arc::clone(&text))),
            );
        }
        self
    }
}

#[cfg(test)]
#[path = "tool_table_host_prefix_tests.rs"]
mod tests;
