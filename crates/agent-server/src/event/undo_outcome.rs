//! [`UndoOutcome`]：一次 undo / redo 的结果——[`agent_core::UndoReport`] 的
//! 可序列化姊妹类型。不改 `UndoReport` 本身：它刻意不 derive `Serialize`
//! （`agent-core` 没有理由知道「落盘/传输需要什么形状」），这里照
//! `agent_runtime::persist::PersistedMeta` 对 `EntryMeta` 的先例另开一个，
//! 字段逐一对应。
//!
//! `tag = "type", content = "data"`：跟 [`super::SessionEvent`] 同一个协议决定，
//! 理由见 `super` 模块文档。
//!
//! 032：`ts` feature 门后面导出 TS——无字段变体（`Nothing`）落成
//! `{ "type": "nothing" }`，没有 `"data"` 键（邻接标签对无内容变体不发 `content`，
//! ts-rs 的 serde-compat 照 serde 的真实序列化形状生成，这一点在实做时用
//! `serde_json::to_string` 实测过，不是读文档猜的）。

use serde::{Deserialize, Serialize};

use agent_core::{BlockedCause as CoreCause, Session, UndoReport};

/// 撤销为什么停下（`agent_core::BlockedCause` 的可序列化姊妹类型，199 §五）。
///
/// **必须传到协议面**，不能在服务端被 `..` 忽略掉：加成因的全部目的就是让用户
/// 分得清「屏障——没碰」和「还原失败——碰了，可能做了一半」，好据此决定要不要
/// force。成因只留在 CLI、不给浏览器/桌面，等于对一半宿主形态白做。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "data")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum BlockedCause {
    /// 这一步没交还原函数——今天的屏障，**没碰**。
    NoHook,
    /// 钩子跑了但失败了——**碰了，可能做了一半**，附一句原因。
    HookFailed(String),
    /// 钩子已随进程重启消失（还原函数是闭包，不跨进程）。
    HookLost,
}

impl From<CoreCause> for BlockedCause {
    fn from(cause: CoreCause) -> Self {
        match cause {
            CoreCause::NoHook => BlockedCause::NoHook,
            CoreCause::HookFailed(why) => BlockedCause::HookFailed(why.to_string()),
            CoreCause::HookLost => BlockedCause::HookLost,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "data")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum UndoOutcome {
    /// 走完了：`entries` 条属于 `turn_id` 这一轮的条目被回滚 / 重放。
    Applied { entries: usize, turn_id: u64 },
    /// 撞上屏障（`Irreversible` 工具），`barrier_seq` 那一条没被越过。
    ///
    /// 034：`label`/`tool`/`call_id` 是富化字段——`agent_core::UndoReport::
    /// Blocked` 只有 `barrier_seq`，`agent-server` 跟 `Session` 活在同一个
    /// 进程里，能现查那条 entry（`Session::barrier_info`），没有理由让 web
    /// 端只看着一个裸数字猜「越过的是什么」（027 的原则）。`label` 恒有值；
    /// `tool`/`call_id` 目前恒为 `Some`（barrier 只会在 tool_result/
    /// tool_failed 那条上置真），`None` 是防御性的兜底，不是已知会走到的分支。
    Blocked {
        entries: usize,
        barrier_seq: u64,
        label: String,
        tool: Option<String>,
        call_id: Option<String>,
        /// 199 §五 的成因。`label`/`tool`/`call_id` 回答「停在哪一条」，这一位
        /// 回答「为什么停」——两者都缺一不可：同一条 `shell/exec` 因「没交还原
        /// 函数」停下和因「还原函数跑挂了」停下，对用户是两件事。
        cause: BlockedCause,
    },
    /// 无可做（游标已在端点）。
    Nothing,
}

impl UndoOutcome {
    /// [`UndoReport`] → [`UndoOutcome`]，`Blocked` 分支现查 `session` 富化
    /// （034）。`Applied`/`Nothing` 字段直接照抄，不需要 `session`。
    ///
    /// 不是 `From<UndoReport>`：那个 trait 拿不到 `session`，而富化偏偏只有
    /// 在这里（actor 线程，同进程持有 `Session`）才做得到——挂在一个具名方法上
    /// 比留一个「大部分时候够用、`Blocked` 时悄悄拿到空字符串」的 `From` 更诚实。
    pub fn from_report(report: UndoReport, session: &Session) -> Self {
        match report {
            UndoReport::Applied { entries, turn_id } => UndoOutcome::Applied { entries, turn_id },
            UndoReport::Nothing => UndoOutcome::Nothing,
            UndoReport::Blocked {
                entries,
                barrier_seq,
                cause,
            } => {
                let info = session.barrier_info(barrier_seq);
                UndoOutcome::Blocked {
                    entries,
                    barrier_seq,
                    label: info
                        .as_ref()
                        .map(|i| i.label.to_string())
                        .unwrap_or_default(),
                    tool: info
                        .as_ref()
                        .and_then(|i| i.tool.as_deref().map(str::to_string)),
                    call_id: info.and_then(|i| i.call_id.map(|c| c.0.to_string())),
                    cause: cause.into(),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use agent_core::{
        AgentId, ContentBlock, Event, PrefixImage, StopReason, TokenUsage, ToolCallId,
    };

    use super::*;

    #[test]
    fn from_report_applied_and_nothing_translate_field_for_field() {
        let session = Session::new(AgentId::root());
        assert_eq!(
            UndoOutcome::from_report(
                UndoReport::Applied {
                    entries: 3,
                    turn_id: 2
                },
                &session
            ),
            UndoOutcome::Applied {
                entries: 3,
                turn_id: 2
            }
        );
        assert_eq!(
            UndoOutcome::from_report(UndoReport::Nothing, &session),
            UndoOutcome::Nothing
        );
    }

    /// 同 `agent-cli::undo` / `agent_core::command::barrier` 那份夹具：一次
    /// `srv:shell/exec` 调用、宿主标记不可逆、结果落地——真实撞出一条
    /// barrier entry。
    fn session_with_a_barrier_entry() -> Session {
        let mut session = Session::new(AgentId::root());
        let _ = session.step(Event::UserInput {
            agent: AgentId::root(),
            text: "跑个命令".into(),
        });
        let call_id = ToolCallId::new("call_shell_1");
        let _ = session.step(Event::ProviderDone {
            agent: AgentId::root(),
            epoch: session.epoch(),
            blocks: vec![ContentBlock::ToolUse {
                id: call_id.clone(),
                name: Arc::from("srv:shell/exec"),
                input: Arc::new(serde_json::json!({"cmd": "echo hi"})),
            }],
            stop: StopReason::ToolUse,
            usage: TokenUsage {
                prompt: 10,
                completion: 5,
                cached: None,
            },
            prefix: PrefixImage {
                segments: Vec::new(),
                prompt_tokens: None,
            },
            adjustments: Vec::new(),
        });
        session.mark_irreversible(call_id.clone());
        let _ = session.step(Event::ToolResult {
            agent: AgentId::root(),
            epoch: session.epoch(),
            call_id,
            content: Arc::from("hi\n"),
        });
        session
    }

    /// 034 的核心断言：撞屏障之后，`from_report` 真的把工具名/call_id 从
    /// `session` 里现查出来，不是留一个空 `Blocked { entries, barrier_seq }`
    /// 让 web 端只能显示一个裸数字。
    #[test]
    fn from_report_blocked_enriches_tool_and_call_id_from_the_session() {
        let mut session = session_with_a_barrier_entry();
        let report = session.undo_turn();
        assert!(matches!(report, UndoReport::Blocked { .. }), "{report:?}");

        let outcome = UndoOutcome::from_report(report, &session);
        let UndoOutcome::Blocked {
            tool,
            call_id,
            label,
            ..
        } = outcome
        else {
            panic!("该是 Blocked：{outcome:?}");
        };
        assert_eq!(tool.as_deref(), Some("srv:shell/exec"));
        assert_eq!(call_id.as_deref(), Some("call_shell_1"));
        assert_eq!(label, "tool_result");
    }
}
