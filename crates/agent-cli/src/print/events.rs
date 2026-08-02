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

use agent_core::{Adjustment, AgentId, GuardReport, Notice, TokenUsage};
use agent_runtime::{AgentEvent, RunnerEvent};

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
        EventPrinter { mode: Mode::None, speaker: None }
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
                println!(
                    "{at}[tool] {} {} (call_id={} location={:?} reversibility={:?})",
                    request.tool, request.input, call_id.0, request.location, request.reversibility
                );
            }
            RunnerEvent::ToolExecuted { call_id, tool, output_len, is_error } => {
                let verdict = if is_error { "失败" } else { "完成" };
                println!("{at}      -> {tool} {verdict}，{output_len} 字节（call_id={}）", call_id.0);
            }
            RunnerEvent::TurnGuard { usage, report, adjustments } => {
                self.finish_line();
                print_turn_guard(&at, &usage, &report, &adjustments);
            }
            RunnerEvent::Notice(notice) => self.handle_notice(&at, notice),
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
            Notice::ToolOutputTruncated { call_id, original_bytes, kept_bytes } => {
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
            Notice::Retrying { attempt, max_retries } => {
                self.finish_line();
                println!("{at}[重试中] {attempt}/{max_retries}");
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

fn print_turn_guard(at: &str, usage: &TokenUsage, report: &GuardReport, adjustments: &[Adjustment]) {
    let cached_str = match usage.cached {
        Some(n) => n.to_string(),
        None => "None（这家没报）".to_string(),
    };
    println!("{at}--- usage: prompt={} completion={} cached={cached_str}", usage.prompt, usage.completion);
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

    /// root 不带前缀（单 agent 的输出跟 M2 一个字不差），子 agent 带短 id，
    /// 且短 id 认得出祖先——`root/a1/a1` 与 `root/a2/a1` 不能打成同一个前缀。
    #[test]
    fn root_has_no_prefix_and_children_are_told_apart() {
        assert_eq!(prefix(&AgentId::root()), "");
        assert_eq!(prefix(&AgentId::new("root/a1")), "[a1] ");
        assert_ne!(prefix(&AgentId::new("root/a1/a1")), prefix(&AgentId::new("root/a2/a1")));
    }
}
