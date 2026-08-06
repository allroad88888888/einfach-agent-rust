//! 专用视觉检查工具的纯协议面。
//!
//! 本模块只固定模型可见的请求、稳定失败码与子 agent 终态的结果形状。附件归属、
//! provider 路由、上传、子 agent 生命周期与超时都由宿主实现；core 不做 IO，也不
//! 接触 endpoint、model 或 key。

mod outcome;
mod request;

pub use outcome::{
    VisionChildTerminal, VisionFailure, VisionFailureCode, VisionToolOutcome, vision_child_outcome,
};
pub use request::{
    MAX_VISION_IMAGES, MAX_VISION_QUESTION_CHARS, VISION_INSPECT_TOOL, VisionImageHandle,
    VisionInspectRequest, parse_vision_inspect_request, vision_inspect_spec,
};
