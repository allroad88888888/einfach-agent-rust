//! 一次 provider 交互的配置与结果元数据：请求前的 `SessionConfig`，
//! 请求后的 `StopReason` 与 `TokenUsage`。

use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// 一次会话对 provider 的固定配置。**这里只放会进请求组装的参数**，不放消息
/// 历史——历史是 `Message` 的事，配置和历史分属两个 atom，undo 时才能各自
/// 独立判断变没变（红线 5：`PartialEq` 决定要不要传播）。
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct SessionConfig {
    pub model: Arc<str>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    /// 上下文窗口大小，单位 token。**压缩触发用它做纯算术**：当前 tokens 数
    /// 与这个值比较，超阈值就触发压缩——纯比较不涉及能力位分支，符合红线 12
    /// （决策 18，docs/ROADMAP.md：「压缩三分」——触发在 core 是纯算术，
    /// 实现在 core，摆盘在 adapter）。`None` 表示未知/不设限，触发逻辑要能
    /// 处理这种情况，不能直接 `unwrap`。
    pub context_window: Option<u32>,
}

/// 一轮生成为什么停下来。
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    StopSequence,
    /// 未知的 finish_reason 原样存下来，**不许猜成 `EndTurn`**（docs/ADAPTER.md）
    /// ——猜错了 loop 会以为轮次正常结束，实际上模型可能是被截断或者出错了。
    ///
    /// 类型是 `Arc<str>` 而不是 issue 描述里的 `&'static str`：后者要求
    /// `Deserialize<'static>`，只有从编译期字符串字面量借用时才成立；从运行时
    /// 数据反序列化（比如 `serde_json::from_str` 一份来自 provider 响应体的
    /// `String`）借不出 `'static` 生命周期，序列化往返测试直接编译不过。
    /// `Arc<str>` 保留「不可变、克隆是指针拷贝」的意图（红线 5），且能正常
    /// serde 往返，这是这里唯一做的取舍。
    Other(Arc<str>),
}

/// 一次请求的 token 用量与缓存命中情况。
///
/// 032：`SessionEvent::TurnGuard.usage` 的类型，`ts` feature 门后面导出 TS。
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct TokenUsage {
    pub prompt: u32,
    pub completion: u32,
    /// **`None` 和 `Some(0)` 语义不同，混同会让缓存兜底第 2 层永远对不上账。**
    ///
    /// `None`：这家 provider 的响应里这个字段整个缺失——它没有报告缓存信息，
    /// 不代表没命中，只是「这家不说」。`Some(0)`：字段存在且取值 0——明确报告
    /// 「这次真的没命中」。probes/PROVIDERS.md 实测：三家里有一家未命中时字段
    /// 整个缺失（对应 `None`），另外两家未命中时给出显式的 `0`（对应
    /// `Some(0)`）。解析时如果把这两种情况都揉成默认值 0，「这家不报」和
    /// 「这家报了未命中」在下游统计里会变成同一件事，缓存命中率的账就对不上。
    pub cached: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_config_roundtrip() {
        let cfg = SessionConfig {
            model: Arc::from("claude-sonnet"),
            temperature: Some(0.7),
            max_tokens: Some(4096),
            context_window: Some(200_000),
        };
        let s = serde_json::to_string(&cfg).unwrap();
        assert_eq!(serde_json::from_str::<SessionConfig>(&s).unwrap(), cfg);
    }

    #[test]
    fn stop_reason_roundtrip() {
        for reason in [
            StopReason::EndTurn,
            StopReason::ToolUse,
            StopReason::MaxTokens,
            StopReason::StopSequence,
            StopReason::Other(Arc::from("weird_reason")),
        ] {
            let s = serde_json::to_string(&reason).unwrap();
            assert_eq!(serde_json::from_str::<StopReason>(&s).unwrap(), reason);
        }
    }

    /// `cached`：`None`（这家没报）与 `Some(0)`（真没命中）是两码事，两条路径
    /// 都要能独立往返，且序列化结果必须不同——否则下游没法区分。
    #[test]
    fn token_usage_cached_none_vs_zero() {
        let usage_none = TokenUsage {
            prompt: 100,
            completion: 20,
            cached: None,
        };
        let s = serde_json::to_string(&usage_none).unwrap();
        assert_eq!(serde_json::from_str::<TokenUsage>(&s).unwrap(), usage_none);

        let usage_zero = TokenUsage {
            prompt: 100,
            completion: 20,
            cached: Some(0),
        };
        let s = serde_json::to_string(&usage_zero).unwrap();
        assert_eq!(serde_json::from_str::<TokenUsage>(&s).unwrap(), usage_zero);

        assert_ne!(
            serde_json::to_string(&usage_none).unwrap(),
            serde_json::to_string(&usage_zero).unwrap()
        );
    }
}
