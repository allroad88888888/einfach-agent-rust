//! **建会话时把部署方给的某几件内置工具藏起来**（076，接缝见
//! `docs/HOST-CAPABILITIES.md` §三之二）。
//!
//! 从 `tool_table.rs` 分出来的一件事，跟隔壁两件（[`host`](super::host) 的注入、
//! [`skill_tools`](super::skill_tools) 的 skill）并列——但**方向相反**：那两件是往表里
//! 加，这一件是从表里减。
//!
//! # 「不启用」的定义只有一个：连名字带描述都不进 prompt
//!
//! 不是「看得见但不给调」（那样模型仍然会去调，然后收到一条它读不懂的拒绝，白花
//! 一轮），也不是「预先激活正文」。剔掉就是剔掉：`specs()` 里没有它，`declares()`
//! 为假，于是模型凭空猜出来的那个名字跟别的不存在的工具走同一条路
//! （`unknown_tool`）——spawn 的截获闸靠的正是 `declares()`，所以关掉
//! `srv:agent/spawn` 之后，即便模型硬猜也长不出一棵子 agent 树。
//!
//! # 三条硬规矩
//!
//! 1. **只能减不能加。** 这个函数只会让表变短。部署方装配出来的那张表是**天花板**，
//!    会话只能在它下面挑——反过来（客户端说「给我开 `srv:shell/exec`」）意味着前端
//!    一句 JSON 就能突破部署方的决定，而这条路上的客户端是浏览器。名字不在表里
//!    时**在更早的地方就该 400**（`agent-server` 的 HTTP 路由，作者还在场的那一刻），
//!    所以这里对认不出的名字**不报错也不做任何事**：每次装表都会跑到它，而那时作者
//!    早就不在场了（跟 064 期那条每轮都跑、作者不在场的过滤逻辑同一条判据）。
//! 2. **排在五档之后、`with_skills`/`with_mcp`/`with_host_tools` 之前。** 减的是
//!    **部署方给的那批**；宿主自己声明的能力不受这个开关影响（那是它自己报进来的，
//!    要不要给就别报）。链式顺序即语义，这一条靠调用点保证
//!    （`agent_server::actor::capabilities::assemble`）。
//! 3. **剩下的那些一个字节不动。** 剔除用 `retain`，保住五档原有的相对次序（红线 11：
//!    既有顺序是契约）。于是「关掉最后一个」这种最常见的情形下，前面那一整段跟不关
//!    的会话**逐字节相同**，前缀还接得上——变的只是尾巴。
//!
//! # 为什么不留一份「被关掉了什么」在表里
//!
//! 试过的另一种形状是给 `ToolTable` 加一个 `disabled` 字段，装配时照旧 push、
//! 渲染时再滤。那会让「表里有什么」和「模型看得见什么」变成两个可能不一致的答案，
//! 而 `declares()` 只能回答其中一个——spawn 的截获闸、069 的撞名过滤、075 的
//! `push_spec` 判重全都问它。**减法必须在进表那一刻就结账**，不能留到渲染。

use std::sync::Arc;

use super::ToolTable;

impl ToolTable {
    /// 076：把 `disabled` 里的名字从这张表里**整条剔掉**（spec 不留，可逆性映射也
    /// 不留）。空 `disabled` 是一次真正的空操作——不带这个字段的会话，工具表跟
    /// 076 之前**逐字节相同**。
    ///
    /// 入参就是 `agent_core::Session::disabled_builtins()` 的产物（排序去重过，
    /// 红线 11 在那一处结账）。这里**不假设它有序**：判据是集合成员关系，同一份
    /// 名单换个顺序、多写一个重复项，出来的表都一样。
    ///
    /// 认不出的名字**静默跳过**（不报错、不 panic）：装表这一步每次开会话都会跑到，
    /// 而「名字得在部署方那张表里」这条闸在 HTTP 路由上，报错要报在那里
    /// （069 §拍板「在最早能报给作者的点上失败」）。
    pub fn without_builtins(mut self, disabled: &[Arc<str>]) -> Self {
        if disabled.is_empty() {
            return self;
        }
        self.specs
            .retain(|spec| !disabled.iter().any(|name| **name == *spec.name));
        // 两张可逆性映射同步剔——理论上此刻它们都还是空的（这个 `with_*` 排在
        // `with_mcp`/`with_host_tools` **之前**），但「表里没有 spec、映射里却还
        // 留着一条」正是 075 的 `push_spec` 花了一整段说明去避免的那种隐式耦合，
        // 不能靠「调用顺序目前是对的」来免掉。
        self.mcp_reversibility
            .retain(|name, _| !disabled.iter().any(|off| off == name));
        self.host_reversibility
            .retain(|name, _| !disabled.iter().any(|off| off == name));
        self
    }
}

#[cfg(test)]
#[path = "tool_table_disable_tests.rs"]
mod tests;
