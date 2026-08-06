//! 视觉子 agent 的 runtime 终态到 core 稳定信封的窄翻译层。
//!
//! provider 分类只在这里解释一次；正文始终经过 `agent_core::vision` 生成，绝不把
//! provider body、route、model 或 key 带回父 agent。

use agent_core::vision::{VisionChildTerminal, vision_child_outcome};
use agent_core::{AgentId, ErrorClass, Failure, Session, TurnStatus};

use crate::child_outcome::final_text;

pub(crate) fn outcome(
    session: &Session,
    child: &AgentId,
    status: &TurnStatus,
    images_inspected: usize,
    timed_out: bool,
) -> (String, bool) {
    let terminal = match status {
        TurnStatus::Done { truncated } => VisionChildTerminal::Succeeded {
            observation: final_text(session, child).into(),
            truncated: *truncated,
        },
        TurnStatus::Failed(Failure::Cancelled) => VisionChildTerminal::Cancelled,
        TurnStatus::Failed(Failure::Provider(ErrorClass::Retryable)) if timed_out => {
            VisionChildTerminal::TimedOut
        }
        TurnStatus::Failed(Failure::Provider(
            ErrorClass::BadRequest | ErrorClass::Auth | ErrorClass::Exhausted,
        )) => VisionChildTerminal::Rejected,
        TurnStatus::Failed(Failure::Provider(ErrorClass::Retryable)) => {
            VisionChildTerminal::Failed { retryable: true }
        }
        TurnStatus::Failed(Failure::Provider(ErrorClass::Unknown)) => {
            VisionChildTerminal::Failed { retryable: false }
        }
        // 收割只在终态发生；保守兜底仍返回安全信封，不能因宿主时序 bug panic。
        TurnStatus::Idle | TurnStatus::Thinking | TurnStatus::ToolsPending => {
            VisionChildTerminal::Failed { retryable: false }
        }
    };
    let outcome = vision_child_outcome(terminal, images_inspected);
    (outcome.content.to_string(), outcome.is_error)
}

#[cfg(test)]
#[path = "vision_child_outcome_tests.rs"]
mod tests;
