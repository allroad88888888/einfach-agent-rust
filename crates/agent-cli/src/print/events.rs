//! 事件流 → 终端：[`EventPrinter`]，一个会话一份的打印状态机。
//!
//! 012 的要求：打印格式保持现有风格（usage / 三层判读 / adjustments），
//! 三层判读换成 [`agent_core::cache::GuardReport`] 的中文 `Display`（024 已经
//! 提供）——不再手写「对不上」那句已知会吓人的措辞。工具调用要可见：
//! 调了什么、参数、结果多长（`ToolExecuting` / `ToolExecuted` 两个分支）。
//!
//! 029：多 agent 并行之后，每件事前面要能看出**谁说的**——回调收的是
//! [`AgentEvent`]（`RunnerEvent` + 归属），前缀由 [`prefix`] 一处决定，
//! **root 不带前缀**，所以单 agent 会话的输出跟 M2 一个字节不差。

use std::io::{self, Write};

use agent_core::{AgentId, Notice};
use agent_runtime::{AgentEvent, RunnerEvent};

use super::event_text::{describe_fate, describe_reversibility, hold_reason, print_turn_guard};

const DIM_ON: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

/// 一整轮（可能横跨多次 `CallProvider` 重试/多个工具调用/多个并行子 agent）的
/// 打印状态机。**每个会话一个**，跨多轮 `run_turn` 调用复用——流式增量的
/// 暗色/常规色切换要在多轮之间保持一致的收尾行为。
///
/// 029 起它还记着「上一条是谁说的」：并行的子 agent 会把增量交错吐出来，
/// 换人就得先收尾当前这行再起一段，否则两个子 agent 的句子会拼进同一行。
pub struct EventPrinter {
    mode: Mode,
    speaker: Option<AgentId>,
}

#[derive(PartialEq)]
enum Mode {
    None,
    Thinking,
    Text,
}

impl EventPrinter {
    pub fn new() -> Self {
        EventPrinter {
            mode: Mode::None,
            speaker: None,
        }
    }

    /// `RunnerCtx` 的事件回调直接调这个方法（`RunnerCtx::with_agent_events`）。
    pub fn handle(&mut self, event: AgentEvent) {
        let AgentEvent { agent, event } = event;
        self.switch_speaker(&agent);
        let at = prefix(&agent);
        match event {
            RunnerEvent::TextDelta(t) => self.text(&t),
            RunnerEvent::ThinkingDelta(t) => self.thinking(&t),
            RunnerEvent::ToolCallStarted { name } => {
                self.finish_line();
                println!("{at}[tool_call] {name} ...(参数流式拼接中)");
            }
            RunnerEvent::PreflightDriftAlert(verdict) => {
                self.finish_line();
                println!("{at}!! {verdict}");
            }
            RunnerEvent::TransportTrouble(message) => {
                self.finish_line();
                eprintln!("{at}[连接异常] {message}");
            }
            RunnerEvent::ToolExecuting { call_id, request } => {
                self.finish_line();
                // `reversibility` 不直接 `{:?}`：宿主与 MCP 工具的那个字要带上
                // 「本仓不代为补偿」（202），判据在 `describe_reversibility`。
                println!(
                    "{at}[tool] {} {} (call_id={} location={:?} reversibility={})",
                    request.tool,
                    request.input,
                    call_id.0,
                    request.location,
                    describe_reversibility(&request)
                );
            }
            RunnerEvent::ToolExecuted {
                call_id,
                tool,
                output_len,
                is_error,
            } => {
                let verdict = if is_error { "失败" } else { "完成" };
                println!(
                    "{at}      -> {tool} {verdict}，{output_len} 字节（call_id={}）",
                    call_id.0
                );
            }
            RunnerEvent::TurnGuard {
                usage,
                report,
                adjustments,
            } => {
                self.finish_line();
                print_turn_guard(&at, &usage, &report, &adjustments);
            }
            RunnerEvent::Notice(notice) => self.handle_notice(&at, notice),
            // 054：轮末孤儿告警的专属变体。归属（`at`）是**父**，出事的子在
            // `child` 里——事件载荷只带事实，句子在这里组（见 `describe_fate`）。
            RunnerEvent::OrphanedChild { child, fate } => {
                self.finish_line();
                eprintln!(
                    "{at}[后台子未领取] {} {}",
                    child.as_str(),
                    describe_fate(&fate)
                );
            }
            // 206：轮末还有话没被读到——**编排失误的信号，不是错误**。
            // 措辞在这里组（载荷只带事实），跟 `OrphanedChild` 一条规矩。
            RunnerEvent::UnreadMessages { agent, count } => {
                self.finish_line();
                eprintln!(
                    "{at}[消息未读] {} 还有 {count} 条没看到——发的时候它多半已经答完了",
                    agent.as_str()
                );
            }
            // 211：这一轮是**留言自己开的**，不是人开的。这条必须显眼——本仓第一次
            // 在没有用户输入的情况下继续烧 token，用户失去的第一样东西是「我知道
            // 现在在干什么」。剩余预算一并打出来，因为那是它还会自己跑几轮的上界。
            RunnerEvent::AutoTurnStarted { remaining } => {
                self.finish_line();
                eprintln!(
                    "{at}[自驱动] 这一轮是留言自己开的（不是你），之后还能自己开 {remaining} 轮。Ctrl-C 随时停，剩下的留言不会丢"
                );
            }
            // 211：有留言等着但没自己开。三种成因都不是错误，但都得说出来
            // ——不说的话「什么都没发生」跟「留言被吞了」在外面长得一模一样。
            RunnerEvent::AutoTurnHeld { pending, reason } => {
                self.finish_line();
                eprintln!("{at}[自驱动] 还有 {pending} 条留言没处理：{}", hold_reason(&reason));
            }
            // 109：压缩点在时间线上可见的两条信号——core 的 `Notice` 那两条
            // （`CompactionSummaryReceived`/`CompactionFailed`）说的是「闸放行
            // 没放行」，这两条带着 runner 才知道的 `upto`/`call_ids`，是「盖住
            // 了哪一段/清了哪些调用」。CLI 只打一行小字，不展开原文——那是 web
            // 时间线（`packages/web/src/render/compaction.ts`）的活，原文要走
            // `GET /sessions/{id}/compaction_record`，CLI 没有这个查询通道。
            RunnerEvent::CompactionApplied {
                turn_id,
                upto,
                summary_id,
            } => {
                self.finish_line();
                println!(
                    "{at}[压缩] 摘要 {} 已生效，覆盖前 {upto} 条消息（turn={turn_id}）",
                    summary_id.as_str()
                );
            }
            RunnerEvent::ToolResultsCleared { turn_id, call_ids } => {
                self.finish_line();
                let ids: Vec<&str> = call_ids.iter().map(|id| id.0.as_ref()).collect();
                println!(
                    "{at}[压缩] 已清除 {} 个工具结果（turn={turn_id}）: {}",
                    call_ids.len(),
                    ids.join(", ")
                );
            }
        }
    }

    /// 换人说话时先把上一行收干净。**只看 agent 换没换**，不看事件种类：
    /// 两个子 agent 的流式增量交错到达时，只有这一下能防止它们的句子拼成一行。
    fn switch_speaker(&mut self, agent: &AgentId) {
        if self.speaker.as_ref() != Some(agent) {
            self.finish_line();
            self.speaker = Some(agent.clone());
        }
    }

    fn handle_notice(&mut self, at: &str, notice: Notice) {
        match notice {
            // 状态变化本身不单独打一行——`run_turn` 的返回值已经是权威的
            // 终态，`turn_outcome` 会打一次收尾摘要，这里再打一遍是同一件事
            // 说两遍。
            Notice::TurnStatusChanged { .. } => {}
            Notice::ToolOutputTruncated {
                call_id,
                original_bytes,
                kept_bytes,
            } => {
                self.finish_line();
                println!(
                    "{at}[截断] call_id={} 原始 {original_bytes} 字节，模型实际看到 {kept_bytes} 字节",
                    call_id.0
                );
            }
            Notice::ProtocolViolation { state, event } => {
                self.finish_line();
                eprintln!("{at}[协议违规] 状态={state:?} 事件={event}");
            }
            Notice::Retrying {
                attempt,
                max_retries,
            } => {
                self.finish_line();
                println!("{at}[重试中] {attempt}/{max_retries}");
            }
            // 105：压缩这一次成没成。打出来是因为它**改变模型看到的历史**，而
            // 看起来完全正常（109 说的「五档里第 3 档是唯一丢了你不知道的」）；
            // 失败那条同样打——不打的话下一轮又全价重编码时没人知道为什么。
            Notice::CompactionSummaryReceived => {
                self.finish_line();
                println!("{at}[压缩] 摘要已接受");
            }
            Notice::CompactionFailed => {
                self.finish_line();
                println!("{at}[压缩] 这一次没做成，历史边界不动");
            }
        }
    }

    fn thinking(&mut self, delta: &str) {
        if self.mode != Mode::Thinking {
            self.enter_line();
            print!("{DIM_ON}");
            self.mode = Mode::Thinking;
        }
        print!("{delta}");
        flush();
    }

    fn text(&mut self, delta: &str) {
        if self.mode != Mode::Text {
            if self.mode == Mode::Thinking {
                print!("{RESET}");
            }
            self.enter_line();
            self.mode = Mode::Text;
        }
        print!("{delta}");
        flush();
    }

    /// 起一段新的增量：先换行（如果上一段还没收尾），再打一次归属前缀。
    /// **前缀只在起段时打一次**，不是每个 delta 都打——delta 是半个词，
    /// 每个都带前缀会把一句话打成一列。
    fn enter_line(&self) {
        if self.mode != Mode::None {
            println!();
        }
        if let Some(agent) = &self.speaker {
            print!("{}", prefix(agent));
        }
    }

    /// 收尾当前这行增量：关掉可能还开着的暗色转义序列，换行，让后面的通报
    /// 另起一段。
    fn finish_line(&mut self) {
        if self.mode == Mode::Thinking {
            print!("{RESET}");
        }
        if self.mode != Mode::None {
            println!();
        }
        self.mode = Mode::None;
    }
}

impl Default for EventPrinter {
    fn default() -> Self {
        Self::new()
    }
}

fn flush() {
    let _ = io::stdout().flush();
}

/// 「谁说的」前缀。**root 不带**（029 原文：单 agent 的输出一个字不变），子 agent
/// 带一个短 id：路径去掉 root 那一段，`root/a1/a2` → `[a1/a2] `。
///
/// 不用「最后一段」当短 id：`root/a1/a1` 和 `root/a2/a1` 的最后一段都是 `a1`，
/// 而那是两个不同的 agent——短到看不出是谁就不叫 id 了。
fn prefix(agent: &AgentId) -> String {
    match agent.as_str().split_once(agent_core::AGENT_PATH_SEP) {
        None => String::new(),
        Some((_root, tail)) => format!("[{tail}] "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// root 不带前缀（单 agent 的输出跟 M2 一个字不差），子 agent 带短 id，
    /// 且短 id 认得出祖先——`root/a1/a1` 与 `root/a2/a1` 不能打成同一个前缀。
    #[test]
    fn root_has_no_prefix_and_children_are_told_apart() {
        assert_eq!(prefix(&AgentId::root()), "");
        assert_eq!(prefix(&AgentId::new("root/a1")), "[a1] ");
        assert_ne!(
            prefix(&AgentId::new("root/a1/a1")),
            prefix(&AgentId::new("root/a2/a1"))
        );
    }
}
