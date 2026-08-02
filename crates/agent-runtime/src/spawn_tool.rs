//! `srv:agent/spawn`：模型用来把一件事拆给子 agent 的那个工具（决策 20）。
//!
//! # 为什么它的声明在 `agent-runtime`，不在 `agent-tools`
//!
//! `agent-tools` 的 `builtin_specs()` 里那几个，全部是 `ToolExecutor::execute`
//! 分发得掉的东西——给一个名字和一份 JSON，它跑完返回一段文本。spawn **不是**
//! 那种工具：它要改的是会话状态（长出一个 agent、记一条 entry），而
//! `ToolExecutor` 既够不着 `Session` 也够不着泵。把它塞进 `builtin_specs()` 只会
//! 得到一个「声明在 A、执行在 B、A 那边永远 `unknown_tool`」的分裂形状。
//!
//! 它的执行点是宿主侧的一次**截获**（`crate::dispatch`：`ExecuteTool` 分派处按
//! 工具名 match），而宿主本来就持有工具表——按名字分流在这一层是合法的，跟
//! 红线 12 禁的「core 里按 provider 分支」不是一回事：这里没有模型相关判断，
//! 只有「这个名字归谁执行」。
//!
//! # 上限进描述，是给模型看的（029：「描述写给模型看」）
//!
//! 决策 20 的兜底是「超限 = `is_error` 的 tool_result 让模型自己收敛」——但先
//! 告诉它上限是多少，能省掉大部分那种往返。数字来自 [`AgentLimits`]，宿主建
//! 工具表时传进来，跟 `Session` 手上那份是同一组数（`ToolTable::with_spawn` 的
//! 文档记了这个耦合）。

use std::sync::Arc;

use agent_core::{AgentLimits, SpawnRefused, ToolSpec};
use serde_json::{Value, json};

/// 工具全名。`srv:` = 服务端本地执行（`Location::Server`，docs/TOOLS.md 的命名
/// 约定），`agent/spawn` = 这一族里的 spawn。
pub const SPAWN_TOOL: &str = "srv:agent/spawn";

/// 喂给模型的声明。
pub fn spawn_spec(limits: AgentLimits) -> ToolSpec {
    ToolSpec {
        name: Arc::from(SPAWN_TOOL),
        description: Arc::from(format!(
            "把一件可以独立完成的子任务交给一个新的子 agent 去做。子 agent 并行工作，\
             它的最终回复会作为这次调用的结果回到你这里。\n\
             什么时候用：一件事能拆成几块互不依赖、各自要读不少材料的子任务时。\
             不要为一次文件读取或一句话回答开子 agent——那比你自己做更慢更贵。\n\
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
                    "description": "允许这个子 agent 使用的工具全名。省略 = 跟你现在一样的这份工具表。只能是你自己有的工具的子集。"
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

    Ok(SpawnRequest { task: Arc::from(task), tools })
}

/// 模型点名了父 agent 自己都没有的工具 → 拒绝，并把「你有哪些」一并告诉它。
///
/// 不静默过滤：静默过滤出来的子 agent 会带着一份跟模型以为的不一样的工具表干活，
/// 然后在子 agent 那边报一个跟 spawn 毫无关系的 `unknown_tool`。
pub(crate) fn check_subset(wanted: &[Arc<str>], parent_has: &[Arc<str>]) -> Result<(), String> {
    let missing: Vec<&str> = wanted
        .iter()
        .filter(|name| !parent_has.iter().any(|have| have == *name))
        .map(|name| &**name)
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    Err(format!(
        "spawn 失败：你要给子 agent 的这些工具你自己没有：{}。你现在有的是：{}。",
        missing.join("、"),
        parent_has.iter().map(|n| &**n).collect::<Vec<_>>().join("、"),
    ))
}

/// [`SpawnRefused`] → 给模型看的一句话。**说清是哪一条闸、当前的数字是多少**，
/// 模型才知道该怎么收敛（决策 20：让它自己收敛）。
pub(crate) fn refusal_text(refused: &SpawnRefused) -> String {
    match refused {
        SpawnRefused::DepthExceeded { depth, max } => format!(
            "spawn 失败：agent 树深度上限是 {max}，这个子 agent 会落在深度 {depth}。\
             这一层不能再往下拆了，剩下的自己做。"
        ),
        SpawnRefused::TooManyChildren { live, max } => format!(
            "spawn 失败：每个 agent 最多 {max} 个活着的直接子 agent，你已经有 {live} 个。\
             等手上这些回来之后再拆，或者少拆几个。"
        ),
        // 下面两条是宿主侧的 bug（父 agent 不在这棵树上 / 已经不活着），不是模型
        // 能收敛的东西——照样如实回给它，让这一轮有个结果而不是卡住。
        SpawnRefused::NotInSession { parent } => {
            format!("spawn 失败：发起 spawn 的 agent（{}）不在这个会话的 agent 树上。", parent.as_str())
        }
        SpawnRefused::ParentNotLive { parent } => {
            format!("spawn 失败：发起 spawn 的 agent（{}）已经不在活名单上了。", parent.as_str())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<Arc<str>> {
        list.iter().map(|n| Arc::from(*n)).collect()
    }

    #[test]
    fn task_is_required_and_must_not_be_blank() {
        assert!(parse(&json!({})).is_err());
        assert!(parse(&json!({ "task": "   " })).is_err());
        assert!(parse(&json!({ "task": 7 })).is_err());
        assert_eq!(&*parse(&json!({ "task": "读一下 a.txt" })).unwrap().task, "读一下 a.txt");
    }

    /// `tools` 缺省与显式 `null` 是同一件事（模型两种都会写）：交给父的子集兜底。
    #[test]
    fn a_missing_tools_field_means_inherit() {
        assert!(parse(&json!({ "task": "t" })).unwrap().tools.is_none());
        assert!(parse(&json!({ "task": "t", "tools": null })).unwrap().tools.is_none());
        let got = parse(&json!({ "task": "t", "tools": ["srv:fs/read"] })).unwrap().tools.unwrap();
        assert_eq!(&*got[0], "srv:fs/read");
    }

    #[test]
    fn tools_must_be_an_array_of_strings() {
        assert!(parse(&json!({ "task": "t", "tools": "srv:fs/read" })).is_err());
        assert!(parse(&json!({ "task": "t", "tools": [1] })).is_err());
    }

    /// 提权被显式拒绝，且拒绝文本里点名缺的是哪一个。
    #[test]
    fn a_child_cannot_be_given_a_tool_the_parent_lacks() {
        let err = check_subset(&names(&["srv:shell/exec"]), &names(&["srv:fs/read"])).unwrap_err();
        assert!(err.contains("srv:shell/exec"), "{err}");
        assert!(err.contains("srv:fs/read"), "{err}");
        assert!(check_subset(&names(&["srv:fs/read"]), &names(&["srv:fs/read", "srv:fs/list"])).is_ok());
    }

    /// 两条闸的文案都得带上当时的数字——只说「超限了」模型不知道该收敛到几。
    #[test]
    fn refusal_text_carries_the_numbers() {
        let text = refusal_text(&SpawnRefused::TooManyChildren { live: 8, max: 8 });
        assert!(text.contains('8'), "{text}");
        let text = refusal_text(&SpawnRefused::DepthExceeded { depth: 4, max: 3 });
        assert!(text.contains('4') && text.contains('3'), "{text}");
    }
}
