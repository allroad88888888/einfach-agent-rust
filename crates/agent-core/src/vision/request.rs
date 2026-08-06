//! `srv:vision/inspect` 的模型声明与严格入参解析。

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{Map, Value, json};

use crate::ToolSpec;

use super::outcome::VisionFailure;

/// 模型调用的专用视觉检查工具名。
pub const VISION_INSPECT_TOOL: &str = "srv:vision/inspect";

/// 一次检查最多选择的图片数。与 093 的工具契约一致。
pub const MAX_VISION_IMAGES: usize = 8;

/// 自包含问题的 Unicode 字符上限。
pub const MAX_VISION_QUESTION_CHARS: usize = 4_096;

const HANDLE_PREFIX: &str = "img_";
const ATTACHMENT_PREFIX: &str = "attachment://";
const MAX_HANDLE_NUMBER: u64 = u64::MAX - 1;

/// 已通过语法校验的模型可见图片句柄。
///
/// 模型只能提交裸 `img_<digits>`；宿主需要写入 [`crate::UserImage`] 时，通过
/// [`attachment_reference`](Self::attachment_reference) 取得 provider-neutral 引用。
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct VisionImageHandle(Arc<str>);

impl VisionImageHandle {
    /// 裸的模型可见句柄，例如 `img_42`。
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// core 历史使用的 provider-neutral 引用，例如 `attachment://img_42`。
    pub fn attachment_reference(&self) -> Arc<str> {
        Arc::from(format!("{ATTACHMENT_PREFIX}{}", self.0))
    }

    fn parse(value: &str) -> Option<Self> {
        let digits = value.strip_prefix(HANDLE_PREFIX)?;
        if digits.is_empty()
            || digits.starts_with('0')
            || !digits.bytes().all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        let number = digits.parse::<u64>().ok()?;
        (number <= MAX_HANDLE_NUMBER).then(|| Self(Arc::from(value)))
    }
}

/// 视觉工具的已校验请求。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct VisionInspectRequest {
    images: Vec<VisionImageHandle>,
    question: Arc<str>,
}

impl VisionInspectRequest {
    pub fn images(&self) -> &[VisionImageHandle] {
        &self.images
    }

    pub fn question(&self) -> &str {
        &self.question
    }
}

/// 喂给模型的静态工具声明。
pub fn vision_inspect_spec() -> ToolSpec {
    ToolSpec {
        name: Arc::from(VISION_INSPECT_TOOL),
        description: Arc::from(
            "Inspect selected user images with an isolated trusted vision agent. The child sees \
             only the selected images and this self-contained question; it receives no conversation \
             history or tools. Copy image handles exactly from the user message. Never pass URLs, \
             paths, provider references, or invented handles.",
        ),
        schema: Arc::new(json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "images": {
                    "type": "array",
                    "description": "Distinct image handles copied exactly from the user message.",
                    "items": {
                        "type": "string",
                        "pattern": "^img_[1-9][0-9]*$"
                    },
                    "minItems": 1,
                    "maxItems": MAX_VISION_IMAGES,
                    "uniqueItems": true
                },
                "question": {
                    "type": "string",
                    "description": "A self-contained question about the selected images.",
                    "minLength": 1,
                    "maxLength": MAX_VISION_QUESTION_CHARS
                }
            },
            "required": ["images", "question"]
        })),
    }
}

/// 严格解析模型入参。所有语法或边界错误都落成稳定的 `invalid_input`。
pub fn parse_vision_inspect_request(input: &Value) -> Result<VisionInspectRequest, VisionFailure> {
    let Some(object) = input.as_object() else {
        return Err(invalid("Vision input must be an object."));
    };
    reject_unknown_fields(object)?;
    let images = parse_images(object.get("images"))?;
    let question = parse_question(object.get("question"))?;
    Ok(VisionInspectRequest { images, question })
}

fn reject_unknown_fields(object: &Map<String, Value>) -> Result<(), VisionFailure> {
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "images" | "question"))
    {
        return Err(invalid("Vision input contains an unsupported field."));
    }
    Ok(())
}

fn parse_images(value: Option<&Value>) -> Result<Vec<VisionImageHandle>, VisionFailure> {
    let Some(Value::Array(items)) = value else {
        return Err(invalid("images must be an array of image handles."));
    };
    if items.is_empty() || items.len() > MAX_VISION_IMAGES {
        return Err(invalid("images must contain between 1 and 8 handles."));
    }
    let mut seen = BTreeSet::new();
    let mut images = Vec::with_capacity(items.len());
    for item in items {
        let Some(value) = item.as_str() else {
            return Err(invalid("Every images item must be a string handle."));
        };
        let Some(handle) = VisionImageHandle::parse(value) else {
            return Err(invalid(
                "Every image handle must be a canonical positive img_<digits> value.",
            ));
        };
        if !seen.insert(Arc::clone(&handle.0)) {
            return Err(invalid("images must not contain duplicate handles."));
        }
        images.push(handle);
    }
    Ok(images)
}

fn parse_question(value: Option<&Value>) -> Result<Arc<str>, VisionFailure> {
    let Some(Value::String(question)) = value else {
        return Err(invalid("question must be a string."));
    };
    if question.trim().is_empty() {
        return Err(invalid("question must not be blank."));
    }
    if question.chars().count() > MAX_VISION_QUESTION_CHARS {
        return Err(invalid("question exceeds the character limit."));
    }
    Ok(Arc::from(question.trim()))
}

fn invalid(message: &'static str) -> VisionFailure {
    VisionFailure::invalid_input(message)
}

#[cfg(test)]
#[path = "request_tests.rs"]
mod tests;
