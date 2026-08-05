//! 流式累积器。**这里的公开签名是 025 的接口契约，不许改**。
//!
//! 实现吃下三处实测差异（probes/PROVIDERS.md §三）：
//! 1. usage 可能在 finish 帧之后另起一帧，且那帧 `choices` 为空——先看 usage
//!    再看 choices，且不要求 choices 存在
//! 2. 有的家 `"content": null` 表示空，有的省略字段——按内容判断，不按字段存在判断
//! 3. 有的家每帧重复 `role: "assistant"`——忽略
//!
//! 工具参数按 `index` 分片流下来，**累加不是覆盖**（见 [`tool_parts`]）。
//!
//! **一帧内的事件顺序**：思考 delta → 文本 delta → `ToolCallStarted` →
//! `Finished` → `UsageReady`。usage 排最后是因为它跟 `Finished` 的关系是
//! 「可能同帧、可能晚一帧」，排最后两种情况的相对顺序就一致了。
//! 同一份 usage 重复出现（Kimi 的 finish 帧和尾帧各带一次）只吐一次。

pub(crate) mod stop;
pub(crate) mod tool_parts;
pub(crate) mod usage;

use std::sync::Arc;

use agent_core::{ContentBlock, StopReason, TokenUsage, ToolCallId};
use serde_json::Value;

use tool_parts::ToolParts;

/// 流式过程中吐给上层的增量事件。
#[derive(Clone, PartialEq, Debug)]
pub enum StreamEvent {
    TextDelta(Arc<str>),
    ThinkingDelta(Arc<str>),
    /// 一次工具调用的声明已完整（拿到 id 和 name）。参数可能还在流。
    ToolCallStarted {
        index: u32,
        id: ToolCallId,
        name: Arc<str>,
    },
    /// 收到 `finish_reason`。usage 可能还没来。
    Finished(StopReason),
    UsageReady(TokenUsage),
    /// 收到 `[DONE]`。
    Done,
}

/// 累积器。**每个请求一个**，按行喂 SSE。
///
/// 三家共享一个实现，各家只差 usage 字段的取值路径（构造参数）——
/// 差异是数据不是行为，所以这是一个类型不是 trait 方法群。
pub struct StreamAccumulator {
    /// usage 对象里 cached token 的取值路径，各家不同（如
    /// `["prompt_cache_hit_tokens"]` vs `["prompt_tokens_details","cached_tokens"]`）。
    /// **路径缺失和值为 0 语义不同**——`TokenUsage::cached` 的 `None`/`Some(0)`。
    cached_paths: &'static [&'static [&'static str]],
    /// wire 上的函数名 → 我们的工具全名。默认原样带出。
    name_from_wire: fn(&str) -> Arc<str>,
    text: String,
    thinking: String,
    tools: ToolParts,
    stop: Option<StopReason>,
    usage: Option<TokenUsage>,
    done: bool,
}

impl StreamAccumulator {
    pub fn new(cached_paths: &'static [&'static [&'static str]]) -> Self {
        StreamAccumulator {
            cached_paths,
            name_from_wire: keep_wire_name,
            text: String::new(),
            thinking: String::new(),
            tools: ToolParts::default(),
            stop: None,
            usage: None,
            done: false,
        }
    }

    /// 装上工具名的还原钩子。**加法，不改 [`Self::new`]**：有的家要把工具名
    /// 转义成 wire 收得下的字符集（见 `deepseek::names`），流式路径吐出去的
    /// 名字必须还原回工具全名，否则 router 按名字找不到工具——而这一层是共享的，
    /// 认不得任何一家，所以还原函数由那家的 `accumulator()` 传进来。
    pub fn with_name_from_wire(mut self, name_from_wire: fn(&str) -> Arc<str>) -> Self {
        self.name_from_wire = name_from_wire;
        self
    }

    /// 喂一行 SSE。非 `data:` 行（注释、心跳、空行）直接忽略；
    /// **无法解析的 `data:` 行也只是忽略**——流到一半因一个畸形帧整轮失败，
    /// 比丢一个 delta 糟得多，真正的失败由 finish/usage 缺失体现。
    pub fn push_line(&mut self, line: &str) -> Vec<StreamEvent> {
        let Some(payload) = line.trim_end().strip_prefix("data:") else {
            return Vec::new();
        };
        let payload = payload.trim();
        if payload == "[DONE]" {
            self.done = true;
            return vec![StreamEvent::Done];
        }
        let Ok(frame) = serde_json::from_str::<Value>(payload) else {
            return Vec::new();
        };
        self.push_frame(&frame)
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    /// 收尾。usage 缺失时给默认值——上层据此知道兜底这一轮失明。
    pub fn finish(self) -> (Vec<ContentBlock>, StopReason, TokenUsage) {
        let mut blocks = Vec::new();
        // 思考排在文本前：三家都是 `reasoning_content` 先流（PROVIDERS.md §三）。
        if !self.thinking.is_empty() {
            blocks.push(ContentBlock::Thinking(self.thinking.into()));
        }
        if !self.text.is_empty() {
            blocks.push(ContentBlock::Text(self.text.into()));
        }
        blocks.extend(self.tools.into_blocks(self.name_from_wire));
        // 没等到 finish_reason 就断了：**不许猜成 `EndTurn`**。
        let stop = self.stop.unwrap_or_else(stop::missing);
        // 没等到 usage：`cached: None` 才是诚实的——不是「没命中」，是「这轮
        // 没人报」，兜底第 2 层据此知道自己失明（见 core 的 `TokenUsage::cached`）。
        let usage = self.usage.unwrap_or(TokenUsage {
            prompt: 0,
            completion: 0,
            cached: None,
        });
        (blocks, stop, usage)
    }

    fn push_frame(&mut self, frame: &Value) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        // usage 先取：它可能挂在一个 `choices` 为空的尾帧上，取值不以 choices 存在为前提。
        let fresh = usage::find(frame).map(|u| usage::parse(u, self.cached_paths));

        if let Some(choices) = frame.get("choices").and_then(Value::as_array) {
            for choice in choices {
                self.push_choice(choice, &mut out);
            }
        }

        if let Some(u) = fresh
            && self.usage.as_ref() != Some(&u)
        {
            self.usage = Some(u.clone());
            out.push(StreamEvent::UsageReady(u));
        }
        out
    }

    fn push_choice(&mut self, choice: &Value, out: &mut Vec<StreamEvent>) {
        if let Some(delta) = choice.get("delta") {
            // `delta.role` 每帧重复（GLM）——忽略，否则文本被污染。
            if let Some(t) = text_of(delta.get("reasoning_content")) {
                self.thinking.push_str(t);
                out.push(StreamEvent::ThinkingDelta(Arc::from(t)));
            }
            if let Some(t) = text_of(delta.get("content")) {
                self.text.push_str(t);
                out.push(StreamEvent::TextDelta(Arc::from(t)));
            }
            if let Some(frags) = delta.get("tool_calls").and_then(Value::as_array) {
                for frag in frags {
                    if let Some(a) = self.tools.absorb(frag) {
                        out.push(StreamEvent::ToolCallStarted {
                            index: a.index,
                            id: ToolCallId::new(a.id),
                            name: (self.name_from_wire)(&a.name),
                        });
                    }
                }
            }
        }
        if let Some(raw) = choice.get("finish_reason").and_then(Value::as_str) {
            let stop = stop::from_wire(raw);
            if self.stop.is_none() {
                self.stop = Some(stop.clone());
            }
            out.push(StreamEvent::Finished(stop));
        }
    }
}

/// `new` 的默认：wire 名原样带出（不认识任何一家的转义规则）。
fn keep_wire_name(name: &str) -> Arc<str> {
    Arc::from(name)
}

/// **字段存在 ≠ 有内容**：`null`（DeepSeek）、省略（Kimi/GLM）、空串（收尾帧）
/// 三种写法都得判成「没有」，否则会吐出空 delta。
fn text_of(field: Option<&Value>) -> Option<&str> {
    field.and_then(Value::as_str).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEEPSEEK: &[&[&str]] = &[&["prompt_cache_hit_tokens"]];

    /// 录制的 DeepSeek 尾帧（probes/results/wire-shape.json `stream.tail`）：
    /// `"content": null` 不产出空 delta、`"content": ""` 也不产出、
    /// finish 与 usage 同帧、`[DONE]` 收尾。
    #[test]
    fn deepseek_recorded_tail() {
        let mut acc = StreamAccumulator::new(DEEPSEEK);
        let ev = acc.push_line(
            r#"data: {"choices":[{"index":0,"delta":{"content":null,"reasoning_content":"。"},"finish_reason":null}],"usage":null}"#,
        );
        assert_eq!(ev, vec![StreamEvent::ThinkingDelta(Arc::from("。"))]);

        let ev = acc.push_line(
            r#"data: {"choices":[{"index":0,"delta":{"content":"好","reasoning_content":null},"finish_reason":null}],"usage":null}"#,
        );
        assert_eq!(ev, vec![StreamEvent::TextDelta(Arc::from("好"))]);

        let ev = acc.push_line(
            r#"data: {"choices":[{"index":0,"delta":{"content":"","reasoning_content":null},"finish_reason":"stop"}],"usage":{"prompt_tokens":18,"completion_tokens":15,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":18}}"#,
        );
        assert_eq!(
            ev,
            vec![
                StreamEvent::Finished(StopReason::EndTurn),
                StreamEvent::UsageReady(TokenUsage {
                    prompt: 18,
                    completion: 15,
                    cached: Some(0)
                }),
            ]
        );

        assert!(!acc.is_done());
        assert_eq!(acc.push_line("data: [DONE]"), vec![StreamEvent::Done]);
        assert!(acc.is_done());

        let (blocks, stop, usage) = acc.finish();
        assert_eq!(
            blocks,
            vec![
                ContentBlock::Thinking(Arc::from("。")),
                ContentBlock::Text(Arc::from("好")),
            ]
        );
        assert_eq!(stop, StopReason::EndTurn);
        assert_eq!(usage.cached, Some(0));
    }

    /// 尾帧 `choices` 为空且带 usage（Kimi 的形状）：拿得到 usage，不 panic；
    /// 同一份 usage 前一帧已经报过，不重复吐。
    #[test]
    fn usage_only_tail_frame_with_empty_choices() {
        let mut acc = StreamAccumulator::new(&[&["prompt_tokens_details", "cached_tokens"]]);
        let ev = acc.push_line(
            r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"stop","usage":{"prompt_tokens":110,"completion_tokens":61,"prompt_tokens_details":{"cached_tokens":110}}}]}"#,
        );
        assert_eq!(ev.len(), 2, "finish + usage：{ev:?}");
        let ev = acc.push_line(
            r#"data: {"choices":[],"usage":{"prompt_tokens":110,"completion_tokens":61,"prompt_tokens_details":{"cached_tokens":110}}}"#,
        );
        assert_eq!(ev, vec![], "同一份 usage 不重复吐");
        let (_, _, usage) = acc.finish();
        assert_eq!(usage.cached, Some(110));
    }

    /// 每帧重复的 `role` 不污染文本；非 data 行与畸形 data 行静默忽略。
    #[test]
    fn repeated_role_and_junk_lines_are_ignored() {
        let mut acc = StreamAccumulator::new(DEEPSEEK);
        assert_eq!(acc.push_line(": keep-alive"), vec![]);
        assert_eq!(acc.push_line(""), vec![]);
        assert_eq!(acc.push_line("event: message"), vec![]);
        assert_eq!(acc.push_line("data: {截断的"), vec![]);
        for frame in [
            r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":"a"}}]}"#,
            r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":"b"}}]}"#,
        ] {
            acc.push_line(frame);
        }
        let (blocks, stop, usage) = acc.finish();
        assert_eq!(blocks, vec![ContentBlock::Text(Arc::from("ab"))]);
        // 流断在半截：stop 落 `Other`，usage 全 0 且 cached 是 None（没人报）。
        assert_eq!(stop, StopReason::Other(Arc::from("missing")));
        assert_eq!(
            usage,
            TokenUsage {
                prompt: 0,
                completion: 0,
                cached: None
            }
        );
    }

    /// `new` 的默认是 wire 名原样带出（装钩子那条路见 `deepseek` 的单测）。
    #[test]
    fn default_keeps_wire_name() {
        let mut acc = StreamAccumulator::new(DEEPSEEK);
        let ev = acc.push_line(
            r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"srv_3Afs_2Fread","arguments":"{}"}}]}}]}"#,
        );
        assert_eq!(
            ev,
            vec![StreamEvent::ToolCallStarted {
                index: 0,
                id: ToolCallId::new("c1"),
                name: Arc::from("srv_3Afs_2Fread"),
            }]
        );
    }
}
