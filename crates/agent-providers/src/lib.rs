//! 模型适配层。接缝定义见 docs/ADAPTER.md，本文件是**公开 API 的唯一出口**。
//!
//! 判据一句话：模型相关的判断 → 这里；不是 → `agent-core`（红线 12）。
//! `encode` 是唯一允许按厂商差异做决策的地方，妥协必须报 `Adjustment`。
//!
//! 零 IO：HTTP 归 `agent-transport`。这层的一切都能对录制帧做单元测试。

use agent_core::{
    Adjustment, ContentBlock, ErrorClass, Message, PrefixImage, RequestIntent, Segment,
    SessionConfig, StopReason, SystemChunk, TokenUsage, ToolSpec,
};
use serde_json::Value;

pub mod deepseek;
pub mod glm;
pub mod kimi;
pub mod stream;
pub(crate) mod wire;

pub use stream::{StreamAccumulator, StreamEvent};

/// 料单：core 供给 adapter 的原材料，**未加工、未合并**（ADAPTER.md §料单）。
///
/// 纯数据引用。`tools` 已按优先级排好（产品判断，跟模型无关）；`late_tools`
/// **不许**与 `tools` 预先合并——有的家能挂消息级零代价，合了就再也分不出来。
pub struct Ingredients<'a> {
    /// 分段的 system，不预拼成一个 `String`。
    pub system: &'a [SystemChunk],
    /// 完成的消息历史。
    pub messages: &'a [Message],
    /// 开轮就在的工具，按优先级降序。
    pub tools: &'a [ToolSpec],
    /// 本轮中途激活的工具。
    pub late_tools: &'a [ToolSpec],
    pub config: &'a SessionConfig,
    pub intent: RequestIntent,
    /// 上一轮的前缀镜像（core 只存不判读），没有则是冷启动。
    pub prev_prefix: Option<&'a PrefixImage>,
}

/// 组装产物：能跨线程带走的一切（决策 16——存在理由是线程边界，不是组装）。
pub struct Encoded {
    /// wire JSON 字节。红线 11：同一份料两次组装必须逐字节相同。
    pub body: Vec<u8>,
    /// 本次请求的前缀镜像，宿主存起来下轮传回 `prev_prefix`。
    pub prefix: PrefixImage,
    /// 对着 `prev_prefix` 算出来的「哪一段漂了」。`None` = 该复用的都没变。
    /// 兜底第 1 层的输出——在花钱之前抓我们自己的序列化 bug。
    pub drift: Option<Segment>,
    /// 这次预计命中多少 token（按这家的匹配语义和块粒度算）。
    /// 兜底第 2 层拿它跟真实 usage 对账。冷启动 / 前缀漂了 = 0。
    pub predicted_cache: u32,
    /// 为这家做过的妥协。**空的时候才叫「原样执行了」。**
    pub adjustments: Vec<Adjustment>,
}

/// 非流式响应的译文：中立结构，core 看不到任何 wire 痕迹。
pub struct Decoded {
    pub blocks: Vec<ContentBlock>,
    pub stop: StopReason,
    pub usage: TokenUsage,
}

/// 一家 provider 的适配器。**全部方法都是纯函数**——不做 IO、不重试、不读时钟。
///
/// 方法数 = 各家真的不一样的**动作**数。只差常量的适配是数据不是方法
/// （所以流式累积器是共享类型，不是 trait 方法群）。
pub trait Provider: Send + Sync {
    /// 组装 + 序列化。**唯一允许做模型相关判断的地方。**
    /// 妥协如实记进 `Encoded::adjustments`，静默妥协是本层头号大忌。
    fn encode(&self, ing: &Ingredients<'_>) -> Encoded;

    /// 响应体 → 中立结构。未知的 `finish_reason` 走 `StopReason::Other`，
    /// **不许猜成 `EndTurn`**——猜错了 loop 会以为轮次正常结束。
    fn decode(&self, body: &Value) -> Decoded;

    /// 新建一个流式累积器（每个请求一个）。
    fn accumulator(&self) -> StreamAccumulator;

    /// HTTP 状态 + 响应体 → 错误分类。先按 `error.type` 判，落不到再按状态码
    /// ——各家状态码分配不一致（PROVIDERS.md §四）。
    fn classify(&self, status: u16, body: &str) -> ErrorClass;
}
