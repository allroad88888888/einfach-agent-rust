//! 接缝词汇：core 与 adapter 之间过界的类型（docs/ADAPTER.md §「类型落在哪个 crate」）。
//!
//! 这些定义在 core 而不在 `agent-providers`，因为 core 的事件与状态要携带它们，
//! 而依赖方向是 providers → core——反着引用编译不过，这正是红线 12 的结构保障。
//!
//! **这里只有词汇，没有判断。** 谁翻译这些词、怎么翻译，全在 adapter。

use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// core 对「这轮想怎么用工具」的意图。**不是** `tool_choice`——那是某一系的
/// wire 字段名，用它当字段名就已经假定了翻译方式（红线 12 的料单命名规则）。
///
/// 翻译由 adapter 做：有的家直接支持指定函数，有的家要先关思考，有的家永久
/// 做不到只能降级——做不到的要报 [`Adjustment`]，不许静默。
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum RequestIntent {
    /// 模型自己决定调不调工具。
    Free,
    /// 这轮必须调**某个**工具（不指定哪个）。
    MustUseTool,
    /// 这轮必须调指定工具（带命名空间全名，如 `srv:fs/read`）。
    MustUse(Arc<str>),
}

/// adapter 为某家模型做过的妥协。**空的时候才叫「原样执行了」。**
///
/// 在 `encode` 时产生（降不降级组装时就知道），宿主随 `ProviderDone` 事件喂进
/// loop——core 不事前问能力，只事后看调整（决策 17）。静默妥协是 adapter 层的
/// 头号大忌：功能正常，只在账单或「模型怎么没调那个工具」上浮出来。
///
/// 032：`SessionEvent::TurnGuard.adjustments` 的元素类型，`ts` feature 门后面
/// 导出 TS。
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum Adjustment {
    /// 想强制指定工具，这家做不到，降级成了别的（如 `required`）。
    ToolChoiceDowngraded { wanted: Arc<str>, used: Arc<str> },
    /// 为了满足强制工具调用，关掉了这家默认开着的思考模式——改变了模型行为，
    /// 必须让人看见。
    ThinkingDisabledForToolChoice,
    /// 这家温度锁死，传入值被覆盖。
    TemperatureOverridden { wanted: f32, used: f32 },
    /// 晚加的工具在这家只能并进顶层，本轮前缀作废，代价约 `est_cost_multiple` 倍。
    LateToolsForcedIntoPrefix { count: u32, est_cost_multiple: f32 },
    /// 工具数超过这家上限，按料单给的优先级从尾部裁掉。
    ToolsTruncated { kept: u32, dropped: u32 },
}

/// 错误分类。adapter 的 `classify` 产出，016 的错误分流按它转移——core 不自己
/// 看状态码（各家分配不一致：有家模型名错误给 404，有家过载给 429）。
///
/// 032：经 `Failure::Provider` 可达（`SessionEvent::Notice` → `TurnStatus::Failed`
/// → `Failure::Provider`），`ts` feature 门后面导出 TS。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum ErrorClass {
    /// 退避后重试可能就好了（限流、过载、5xx）。
    Retryable,
    /// 请求本身不合法，重试无意义。
    BadRequest,
    /// 鉴权失败，重试无意义，要人换 key。
    Auth,
    /// **余额耗尽。必须单列**：退避重试毫无意义且要立刻告警到人——
    /// 混进限流会让系统安静地退避到天荒地老。
    Exhausted,
    /// 没认出来。保守处理，不自动重试。
    Unknown,
}

/// 前缀的段。顺序 `[Tools][System][History]` 是三家实测的渲染顺序
/// （probes/PROVIDERS.md §一），不能改——改了整个缓存前缀失效。
///
/// 032：经 `DriftVerdict` 可达，`ts` feature 门后面导出 TS。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum Segment {
    Tools,
    System,
    History,
}

/// 一段前缀的镜像：这段序列化出来多少字节、哈希是什么。
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SegmentImage {
    pub segment: Segment,
    /// 本段字节数（非累计）。
    pub bytes: u32,
    /// 本段字节的哈希。算法是 adapter 的内部约定，core 只比相等。
    pub hash: u64,
}

/// 上一次请求的前缀镜像。**core 只存、只原样传回料单，不判读**——
/// 「哪一段漂了」要对着新请求的原始字节算，只有 adapter 干得了（红线 12）。
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct PrefixImage {
    /// 按 `[Tools][System][History]` 顺序。
    pub segments: Vec<SegmentImage>,
    /// 这次请求实际的 `prompt_tokens`，宿主拿到 usage 后回填。
    /// 兜底第 2 层的预测输入：严格延长时，上一轮的整个 prompt 都该命中
    /// （按块粒度向下取整）——字节数换算不出 token 数，实测值才可靠。
    pub prompt_tokens: Option<u32>,
}

/// system prompt 的一段。**分段交给 adapter，不预先拼成一个 `String`**——
/// 拼好了 adapter 就没得选了（料单规则：宁可分，不可合）。
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SystemChunk {
    /// 段的来源标识：`base` / skill 名 / 等。进日志用，不进 prompt。
    pub label: Arc<str>,
    pub text: Arc<str>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_roundtrip() {
        let adj = vec![
            Adjustment::ToolChoiceDowngraded {
                wanted: Arc::from("srv:fs/read"),
                used: Arc::from("required"),
            },
            Adjustment::ThinkingDisabledForToolChoice,
            Adjustment::LateToolsForcedIntoPrefix { count: 3, est_cost_multiple: 120.0 },
        ];
        let s = serde_json::to_string(&adj).unwrap();
        assert_eq!(serde_json::from_str::<Vec<Adjustment>>(&s).unwrap(), adj);

        let img = PrefixImage {
            segments: vec![SegmentImage { segment: Segment::Tools, bytes: 512, hash: 42 }],
            prompt_tokens: Some(2432),
        };
        let s = serde_json::to_string(&img).unwrap();
        assert_eq!(serde_json::from_str::<PrefixImage>(&s).unwrap(), img);

        let intent = RequestIntent::MustUse(Arc::from("srv:fs/read"));
        let s = serde_json::to_string(&intent).unwrap();
        assert_eq!(serde_json::from_str::<RequestIntent>(&s).unwrap(), intent);
    }
}
