//! 浏览器形态的工具表：**从空表起步，只声明宿主自己能执行的 `web:` 工具**。
//!
//! # 为什么是 `ToolTable::empty()` 而不是 `standard_local()` 减几件
//!
//! 111 决策表第二行：`agent-tools` 的 `srv:` shell/fs specs 在浏览器构建里
//! **不声明**——那些 spec 是纯数据（`pub fn shell_spec()` 只是转发），不声明
//! 模型就压根不知道有它。`ToolTable::empty()` 的文档写着同一条：「公开改写等
//! 不可信输出路径必须从空表开始，不能先装部署期工具再靠名称黑名单回减」。
//! 黑名单回减错一个名字就是一件本不该出现的工具漏进了 prompt，而且**不报错**。
//!
//! 于是 114 验收第五条（「`srv:` 工具不出现在 prompt 里」）在这里是**结构性
//! 成立**的，不靠过滤：这张表里从来没有过 `srv:` 前缀的东西。
//!
//! # 两个工具，为什么正好是这两个
//!
//! 验收第二条要「一个只有前端拿得到的能力」。页面标题与地址栏 URL 就是这类
//! 东西的最小样本：服务端形态的 agent 无论如何都算不出来，只有跑在页面里的
//! 宿主能读。执行在 [`crate::host_tool`]。
//!
//! # 红线 11
//!
//! 工具表进 prompt 最前面，序列化必须逐字节确定。这里两条保证：
//! 1. 表的内容是**编译期固定的常量**，不随会话、时间、随机 id 变化；
//! 2. `ToolTable::with_host_tools` 自己会按名字排序再进表（062，见其模块
//!    文档）——所以哪怕以后有人调乱了 [`host_tools`] 里的书写顺序，进 prompt
//!    的字节也不变。
//!
//! 「刷新页面后第一轮的工具表与关闭前最后一轮逐字节相同」这条验收，靠的就是
//! 这两条；[`tool_table_json`] 把这张表原样吐出来，好让页面（和验收的人）能
//! 直接做字节比对，不必去猜。

use agent_core::{Reversibility, ToolSpec};
use agent_runtime::ToolTable;
use serde_json::json;
use std::sync::Arc;

/// 读 `document.title`。
pub(crate) const PAGE_TITLE_TOOL: &str = "web:page/title";
/// 读 `location.href`。
pub(crate) const PAGE_URL_TOOL: &str = "web:page/url";

/// 这个宿主给模型的全部工具。见模块文档——**没有 `srv:`，没有 `mcp:`**。
pub(crate) fn browser_tool_table() -> ToolTable {
    ToolTable::empty().with_host_tools(host_tools())
}

/// 两条声明 + 各自的可逆性。两个都是纯读，所以 `Pure`——`/undo` 撞上它们不用
/// 停下来问。宿主没说的一律该落保守的 `Irreversible`（HOST-CAPABILITIES §五：
/// 「没说」不能推定为「安全」），这里是明确说了。
fn host_tools() -> Vec<(ToolSpec, Reversibility)> {
    vec![
        (
            ToolSpec {
                name: Arc::from(PAGE_TITLE_TOOL),
                description: Arc::from(
                    "读取当前浏览器页面的标题（document.title）。这个信息只有跑在页面里的宿主拿得到，服务端算不出来。无参数。",
                ),
                schema: Arc::new(empty_object_schema()),
            },
            Reversibility::Pure,
        ),
        (
            ToolSpec {
                name: Arc::from(PAGE_URL_TOOL),
                description: Arc::from(
                    "读取当前浏览器页面的地址（location.href）。这个信息只有跑在页面里的宿主拿得到。无参数。",
                ),
                schema: Arc::new(empty_object_schema()),
            },
            Reversibility::Pure,
        ),
    ]
}

/// 无参数工具的 schema。写成一个函数而不是两处各抄一遍，是因为它会进 prompt
/// ——两份字面量哪天被改歪一个，前缀就在没人察觉的情况下漂了（红线 11）。
fn empty_object_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false
    })
}

/// 把工具表原样序列化成 JSON——**验收第三/第五条的证据面**：页面可以在关闭前
/// 和重开后各取一次做字节比对，也可以一眼看清里面没有 `srv:` 前缀的东西。
///
/// 这不是「进 prompt 的那份字节」本身（那由各家 adapter 的 `encode` 决定），
/// 但它是 `Ingredients::tools` 的输入，两次相同则那份字节必然相同。
pub(crate) fn tool_table_json(table: &ToolTable) -> String {
    serde_json::to_string_pretty(table.specs())
        .unwrap_or_else(|_| "[\"工具表序列化失败\"]".to_string())
}
