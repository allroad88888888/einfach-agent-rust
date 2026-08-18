//! 事件载荷 → 人话：两个纯函数，[`events`](super::events) 拆出来的那一半
//! （109，`events.rs` 顶着行数天花板）。跟 `events` 的分界是 [`super`] 文档写
//! 的那条老规矩——「有没有状态」：[`EventPrinter`](super::EventPrinter) 是个
//! 状态机，这里两个函数都不持状态，纯粹是「给一份事件载荷，吐一句人话」。

use agent_core::{Adjustment, GuardReport, TokenUsage, ToolCallRequest};
use agent_runtime::OrphanFate;

/// 一次工具调用的 `reversibility` 在终端上该怎么写（202，决策 199 §八）。
///
/// **有一格不能只印那个枚举名**：宿主 `web:`/`desk:`（以及 MCP）声明 `reversible`
/// 的时候。那句话是一个**承诺**——「有补偿动作」——而它的执行体在别人的进程里，
/// 那个补偿动作**没有任何人会执行**。终端上印一个孤零零的 `Reversible`，用户读到
/// 的是「这条撤得掉」，事实却是 `/undo` 撞上它会停下来问。**那个字正是 199 现状
/// 清账里骗人的那个字。**
///
/// **其余每一格都只印枚举名**，包括同样够不着的 `Pure`：它声明的是「没碰外部世界」
/// 这个**事实**，从没承诺过补偿，挂一句「本仓不代为补偿」等于说一件它没说过的事
/// 没被兑现。`Irreversible` 同理——它压根没承诺。
///
/// 判据用 [`agent_runtime::is_unkeepable_promise`]，跟 `dispatch` 第四/第五路的
/// **行为**是同一个函数，不是这里另判一遍：行为改了而文案没跟上，就又回到 199 要
/// 修的那个形状。
pub(super) fn describe_reversibility(request: &ToolCallRequest) -> String {
    if agent_runtime::is_unkeepable_promise(request) {
        format!("{:?}（声明，本仓不代为补偿）", request.reversibility)
    } else {
        format!("{:?}", request.reversibility)
    }
}

/// [`OrphanFate`] 的可读呈现（054）。跟 `print::agent_tree::describe_activity`
/// 同一条规矩：事实由 `agent-runtime` 定，措辞由看的人组——web 端那份在
/// `packages/web/src/render/notice.ts`，两处该说同一件事，但各自按自己的排版说。
pub(super) fn describe_fate(fate: &OrphanFate) -> String {
    match fate {
        OrphanFate::Despawned { descendants: 0 } => {
            "还在跑，这一轮收尾时被拆掉了；它在飞的那次调用回来会被丢弃。".to_string()
        }
        OrphanFate::Despawned { descendants } => format!(
            "还在跑，这一轮收尾时连同它的 {descendants} 个后代一起被拆掉了；\
             它在飞的那次调用回来会被丢弃。"
        ),
        OrphanFate::Kept { reason } => {
            format!("没能在这一轮收尾时拆掉（{reason}），它会以活着的状态留到下一轮。")
        }
        OrphanFate::Discarded { bytes, is_error } => {
            let how = if *is_error {
                "失败收场"
            } else {
                "干完了"
            };
            format!("已经{how}，但这一轮结束前没有人 collect 它，{bytes} 字节的结果被丢弃。")
        }
    }
}

pub(super) fn print_turn_guard(
    at: &str,
    usage: &TokenUsage,
    report: &GuardReport,
    adjustments: &[Adjustment],
) {
    let cached_str = match usage.cached {
        Some(n) => n.to_string(),
        None => "None（这家没报）".to_string(),
    };
    println!(
        "{at}--- usage: prompt={} completion={} cached={cached_str}",
        usage.prompt, usage.completion
    );
    println!("{report}");
    if adjustments.is_empty() {
        println!("    adjustments: 无（原样执行了）");
    } else {
        println!("    adjustments:");
        for a in adjustments {
            println!("      - {a:?}");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use agent_core::{Location, Reversibility};

    use super::*;

    fn request(tool: &str, location: Location, reversibility: Reversibility) -> ToolCallRequest {
        ToolCallRequest {
            tool: Arc::from(tool),
            input: Arc::new(serde_json::json!({})),
            location,
            reversibility,
        }
    }

    /// 202 的显示验收：宿主声明 `reversible` 时，打印出来的那一行必须**含有
    /// 「本仓不代为补偿」这层意思**。断言子串而不是完整文案——文案可以改，
    /// 「不许只印一个 `Reversible` 就完事」这条不能改。
    #[test]
    fn a_host_tool_declaring_reversible_says_we_do_not_compensate() {
        let line = describe_reversibility(&request(
            "web:crm/draft",
            Location::Web,
            Reversibility::Reversible,
        ));
        assert!(line.contains("Reversible"), "{line}");
        assert!(line.contains("不代为补偿"), "{line}");
    }

    /// 声明 `pure` 的宿主工具、以及 `readOnlyHint: true` 的 MCP 工具**不带**
    /// 这句话（199 §七 的修正）：它们声明的是「没碰外部世界」这个事实，从没承诺
    /// 过补偿，说「本仓不代为补偿」等于回答一个没人问过的问题——而且它们本来就
    /// 不挡 undo，挂一句免责声明会让用户以为撤销出了什么问题。
    #[test]
    fn pure_host_and_mcp_tools_get_no_caveat_because_they_promised_nothing() {
        for req in [
            request("web:demo/page-title", Location::Web, Reversibility::Pure),
            request("ask_user_question", Location::Web, Reversibility::Pure),
            request("mcp:everything/echo", Location::Server, Reversibility::Pure),
        ] {
            assert_eq!(describe_reversibility(&req), "Pure", "{}", req.tool);
        }
    }

    /// 本进程内的工具不加后缀，**连 `reversible` 也不**：那句话对它们是假的
    /// （`srv:agent/spawn` 的还原由 store 回滚本身完成，`ext:` 的由执行体交回来的
    /// 函数完成），而且每一行都挂一句免责声明会把真正需要注意的那一格淹掉。
    #[test]
    fn in_process_tools_print_the_bare_label() {
        assert_eq!(
            describe_reversibility(&request(
                "srv:fs/read",
                Location::Server,
                Reversibility::Pure
            )),
            "Pure"
        );
        assert_eq!(
            describe_reversibility(&request(
                "srv:agent/spawn",
                Location::Server,
                Reversibility::Reversible
            )),
            "Reversible"
        );
    }

    /// 054：三种收场各说各的话，且**互不相同**——一个 `_ =>` 兜底把三种说成
    /// 一句的话，面板上「被拆了」和「跑完没人领」就分不出来了，而这两件事对
    /// 模型编排的含义完全不同。
    #[test]
    fn each_orphan_fate_reads_differently() {
        let despawned = describe_fate(&OrphanFate::Despawned { descendants: 2 });
        let alone = describe_fate(&OrphanFate::Despawned { descendants: 0 });
        let kept = describe_fate(&OrphanFate::Kept {
            reason: "StillRead".to_string(),
        });
        let discarded = describe_fate(&OrphanFate::Discarded {
            bytes: 15,
            is_error: false,
        });
        let failed = describe_fate(&OrphanFate::Discarded {
            bytes: 15,
            is_error: true,
        });

        assert!(despawned.contains("2 个后代"), "{despawned}");
        assert!(!alone.contains("后代"), "没有后代就别说后代：{alone}");
        assert!(
            kept.contains("StillRead") && kept.contains("留到下一轮"),
            "{kept}"
        );
        assert!(
            discarded.contains("15 字节") && discarded.contains("干完了"),
            "{discarded}"
        );
        assert!(failed.contains("失败收场"), "{failed}");

        let all = [&despawned, &alone, &kept, &discarded, &failed];
        for (i, a) in all.iter().enumerate() {
            for b in all.iter().skip(i + 1) {
                assert_ne!(a, b, "四种收场不该渲染成同一句话");
            }
        }
    }
}
