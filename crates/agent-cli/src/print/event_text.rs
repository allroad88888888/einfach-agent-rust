//! 事件载荷 → 人话：两个纯函数，[`events`](super::events) 拆出来的那一半
//! （109，`events.rs` 顶着行数天花板）。跟 `events` 的分界是 [`super`] 文档写
//! 的那条老规矩——「有没有状态」：[`EventPrinter`](super::EventPrinter) 是个
//! 状态机，这里两个函数都不持状态，纯粹是「给一份事件载荷，吐一句人话」。

use agent_core::{Adjustment, GuardReport, TokenUsage};
use agent_runtime::OrphanFate;

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
    use super::*;

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
