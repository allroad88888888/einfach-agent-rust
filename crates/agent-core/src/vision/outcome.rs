//! 视觉检查的稳定终态码与 tool-result 信封。

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;

/// 视觉检查失败的稳定机器码。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisionFailureCode {
    InvalidInput,
    AttachmentNotFound,
    AttachmentUnavailable,
    ImageUnsupported,
    VisionProfileUnavailable,
    VisionUploadFailed,
    VisionTimeout,
    VisionRejected,
    VisionChildFailed,
    VisionCancelled,
}

/// 一次视觉检查的安全失败描述。
///
/// 构造器只接收固定文案，避免把 provider body、endpoint、key 或原始引用带回 prompt。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct VisionFailure {
    code: VisionFailureCode,
    message: &'static str,
    retryable: bool,
}

impl VisionFailure {
    pub(crate) fn invalid_input(message: &'static str) -> Self {
        Self::new(VisionFailureCode::InvalidInput, message, false)
    }

    pub fn attachment_not_found() -> Self {
        Self::fixed(
            VisionFailureCode::AttachmentNotFound,
            "A selected image was not found in this session.",
            false,
        )
    }

    pub fn attachment_unavailable() -> Self {
        Self::fixed(
            VisionFailureCode::AttachmentUnavailable,
            "A selected image is no longer available.",
            false,
        )
    }

    pub fn image_unsupported() -> Self {
        Self::fixed(
            VisionFailureCode::ImageUnsupported,
            "A selected image format or payload is unsupported.",
            false,
        )
    }

    pub fn profile_unavailable() -> Self {
        Self::fixed(
            VisionFailureCode::VisionProfileUnavailable,
            "The trusted vision execution profile is unavailable.",
            true,
        )
    }

    pub fn upload_failed() -> Self {
        Self::fixed(
            VisionFailureCode::VisionUploadFailed,
            "A selected image could not be prepared for vision inspection.",
            true,
        )
    }

    pub fn timeout() -> Self {
        Self::fixed(
            VisionFailureCode::VisionTimeout,
            "The vision inspection exceeded its deadline.",
            true,
        )
    }

    pub fn rejected() -> Self {
        Self::fixed(
            VisionFailureCode::VisionRejected,
            "The vision inspection was rejected by provider or policy.",
            false,
        )
    }

    pub fn child_failed(retryable: bool) -> Self {
        Self::fixed(
            VisionFailureCode::VisionChildFailed,
            "The vision child failed without a usable observation.",
            retryable,
        )
    }

    pub fn cancelled() -> Self {
        Self::fixed(
            VisionFailureCode::VisionCancelled,
            "The vision inspection was cancelled.",
            false,
        )
    }

    pub fn code(&self) -> VisionFailureCode {
        self.code
    }

    pub fn message(&self) -> &'static str {
        self.message
    }

    pub fn retryable(&self) -> bool {
        self.retryable
    }

    fn fixed(code: VisionFailureCode, message: &'static str, retryable: bool) -> Self {
        Self::new(code, message, retryable)
    }

    fn new(code: VisionFailureCode, message: &'static str, retryable: bool) -> Self {
        Self {
            code,
            message,
            retryable,
        }
    }
}

/// runtime 已判定过的视觉子 agent 终态；这里不解释 provider 状态码。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum VisionChildTerminal {
    Succeeded {
        observation: Arc<str>,
        truncated: bool,
    },
    TimedOut,
    Rejected,
    Failed {
        retryable: bool,
    },
    Cancelled,
}

/// 可直接写回一次 tool slot 的正文与错误位。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct VisionToolOutcome {
    pub content: Arc<str>,
    pub is_error: bool,
}

impl VisionToolOutcome {
    pub fn failure(failure: VisionFailure) -> Self {
        let content = json!({
            "error": {
                "code": failure.code,
                "message": failure.message,
                "retryable": failure.retryable,
            }
        })
        .to_string();
        Self {
            content: Arc::from(content),
            is_error: true,
        }
    }
}

/// 把 runtime 已归一化的子终态纯函数式地翻成稳定工具结果。
pub fn vision_child_outcome(
    terminal: VisionChildTerminal,
    images_inspected: usize,
) -> VisionToolOutcome {
    match terminal {
        VisionChildTerminal::Succeeded {
            observation,
            truncated,
        } => VisionToolOutcome {
            content: Arc::from(
                json!({
                    "observation": observation,
                    "metadata": {
                        "images_inspected": images_inspected,
                        "truncated": truncated,
                    }
                })
                .to_string(),
            ),
            is_error: false,
        },
        VisionChildTerminal::TimedOut => VisionToolOutcome::failure(VisionFailure::timeout()),
        VisionChildTerminal::Rejected => VisionToolOutcome::failure(VisionFailure::rejected()),
        VisionChildTerminal::Failed { retryable } => {
            VisionToolOutcome::failure(VisionFailure::child_failed(retryable))
        }
        VisionChildTerminal::Cancelled => VisionToolOutcome::failure(VisionFailure::cancelled()),
    }
}

#[cfg(test)]
#[path = "outcome_tests.rs"]
mod tests;
