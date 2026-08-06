//! `srv:agent/spawn` 的模型契约：工具声明与入参解析。
//!
//! 截获、子集校验与子树生命周期在 [`crate::spawn_tool`]；本模块只把模型看到和
//! 模型提交的请求固定下来，避免执行路径再承担 schema/parser 的职责。

use std::sync::Arc;

use agent_core::{AgentLimits, ToolSpec};
use serde_json::{Value, json};

/// 工具全名。`srv:` = 服务端本地执行（`Location::Server`，docs/TOOLS.md 的命名
/// 约定），`agent/spawn` = 这一族里的 spawn。
pub const SPAWN_TOOL: &str = "srv:agent/spawn";

/// 喂给模型的声明。
pub fn spawn_spec(limits: AgentLimits) -> ToolSpec {
    ToolSpec {
        name: Arc::from(SPAWN_TOOL),
        description: Arc::from(format!(
            "把一件可以独立完成的子任务交给一个新的子 agent 去做。子 agent 并行工作。\n\
             什么时候用：一件事能拆成几块互不依赖、各自要读不少材料的子任务时。\
             不要为一次文件读取或一句话回答开子 agent——那比你自己做更慢更贵。\n\
             background=false（缺省）：这次调用**等它干完**，它的最终回复就是这次调用的结果。\n\
             background=true：这次调用**立刻**只返回一个 agent_id（不等它干完），你可以接着\
             做别的事、用 srv:agent/status 看它在干啥。**它的回答不会自己回到你这里，必须用 \
             srv:agent/collect 把它领回来**；你这一轮结束前没领的会被拆掉、结果丢弃。\n\
             上限：agent 树深度最多 {}（你在 root 时是 0），每个 agent 最多同时有 {} 个\
             活着的直接子 agent。超了这次调用会返回错误，那时请自己收敛（少拆几个，\
             或者自己做）。",
            limits.max_depth, limits.max_children,
        )),
        schema: Arc::new(json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "交给子 agent 的任务，要能被独立看懂：它看不到你和用户的对话。"
                },
                "tools": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "允许这个子 agent 使用的工具名，照抄你工具列表里的那个名字即可。省略 = 跟你现在一样的这份工具表。只能是你自己有的工具的子集。"
                },
                "background": {
                    "type": "boolean",
                    "description": "true = 不等它干完，这次调用立刻只返回它的 agent_id，它的回答不会自己回来，得用 srv:agent/collect 领（这一轮结束前没领的会被拆掉）；false（缺省）= 等它干完，它的回答就是这次调用的结果。"
                }
            },
            "required": ["task"]
        })),
    }
}

/// 模型给的入参解析结果。
pub(crate) struct SpawnRequest {
    pub(crate) task: Arc<str>,
    /// `None` = 模型没指定，用父的工具子集兜底。
    pub(crate) tools: Option<Vec<Arc<str>>>,
    /// 052：`true` = 后台子 agent——spawn 槽当场收敛成一个 `agent_id`，父不被挡。
    /// **缺省 `false`**（决策 20 的阻塞语义一字不改），所以老模型、老脚本、老录制
    /// 帧走的还是原来那条路。
    pub(crate) background: bool,
}

/// 解析入参。**错误一律是给模型看的文本**（`is_error` 的 tool_result），不是
/// panic 也不是宿主日志：入参是模型写的，写错了该让它自己看见并改（003 的哲学）。
pub(crate) fn parse(input: &Value) -> Result<SpawnRequest, String> {
    let Some(task) = input.get("task").and_then(Value::as_str) else {
        return Err("spawn 失败：缺少必填参数 task（字符串）。".to_string());
    };
    if task.trim().is_empty() {
        return Err("spawn 失败：task 是空的，子 agent 不知道要做什么。".to_string());
    }

    let tools = match input.get("tools") {
        None | Some(Value::Null) => None,
        Some(Value::Array(items)) => {
            let mut names = Vec::with_capacity(items.len());
            for item in items {
                let Some(name) = item.as_str() else {
                    return Err("spawn 失败：tools 里每一项都得是工具全名字符串。".to_string());
                };
                names.push(Arc::from(name));
            }
            Some(names)
        }
        Some(_) => return Err("spawn 失败：tools 得是字符串数组。".to_string()),
    };

    // 缺省与显式 `null` 都是「前台」（模型两种都会写）。**不接受 `"true"` 这种
    // 字符串**：静默把它当成 true，模型就永远不知道自己写错了类型，而这个字段
    // 的两个取值是两套完全不同的语义（等 vs 不等）。
    let background = match input.get("background") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(_) => return Err("spawn 失败：background 得是 true 或 false。".to_string()),
    };

    Ok(SpawnRequest {
        task: Arc::from(task),
        tools,
        background,
    })
}

#[cfg(test)]
#[path = "spawn_request_tests.rs"]
mod tests;
