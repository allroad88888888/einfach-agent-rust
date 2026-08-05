//! Kimi 已上传图片历史的缓存预测。
//!
//! 文本历史按 256 token 块预测。真机记录显示：已在上一请求前缀中的 Kimi
//! `ms://` 图片引用会让服务端按视觉块（1834 tokens）计 cached token，而不是按
//! 上一轮完整 prompt 向下取整。这个判断只在 Kimi adapter 内做；`agent_core` 的图片
//! 引用仍保持不透明，公共 wire 也不带厂商分支。

use agent_core::{PrefixImage, Segment};
use serde_json::Value;

use crate::wire::canonical;

const UPLOADED_IMAGE_PREFIX: &str = "ms://";
/// Kimi 对一张 probe 尺寸的已上传图片计入历史 cache 的视觉块大小。文件引用是
/// 不透明的，不能从 JSON 字节或上轮总 prompt 反推出这个服务端 token 数。
const HISTORY_IMAGE_CACHE_TOKENS: u32 = 1834;

/// 有已处于上一轮 History 前缀内的 Kimi 上传图片时，返回实测的视觉块预测；
/// 冷启动、漂移、图片仅在本轮新增时都交回普通块粒度预测。
pub(super) fn prediction(
    history: &[Value],
    prev: Option<&PrefixImage>,
    drift: Option<Segment>,
) -> Option<u32> {
    if drift.is_some() {
        return None;
    }
    let prev = prev?;
    let _measured_prompt = prev.prompt_tokens?;
    let prior_history_bytes = prev
        .segments
        .iter()
        .find(|segment| segment.segment == Segment::History)?
        .bytes as usize;

    let mut encoded_end = 0;
    for message in history {
        encoded_end += canonical(message).len();
        if encoded_end > prior_history_bytes {
            return None;
        }
        if has_uploaded_image(message) {
            return Some(HISTORY_IMAGE_CACHE_TOKENS);
        }
    }
    None
}

fn has_uploaded_image(message: &Value) -> bool {
    message
        .get("content")
        .and_then(Value::as_array)
        .is_some_and(|parts| {
            parts.iter().any(|part| {
                part.get("type").and_then(Value::as_str) == Some("image_url")
                    && part
                        .pointer("/image_url/url")
                        .and_then(Value::as_str)
                        .is_some_and(|url| url.starts_with(UPLOADED_IMAGE_PREFIX))
            })
        })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use agent_core::{ContentBlock, Message, MessageId, Role};

    use crate::kimi::encode::encode;
    use crate::kimi::test_support::ing;

    fn message(id: u64, role: Role, blocks: Vec<ContentBlock>) -> Message {
        Message {
            id: MessageId(id),
            role,
            blocks,
        }
    }

    fn uploaded_image() -> ContentBlock {
        ContentBlock::Image {
            reference: Arc::from("ms://file-123"),
            mime: Arc::from("image/png"),
            name: Some(Arc::from("probe.png")),
        }
    }

    #[test]
    fn prior_uploaded_image_predicts_the_recorded_vision_cache_block() {
        let image_turn = message(
            1,
            Role::User,
            vec![
                ContentBlock::Text(Arc::from("请读出图片中的数字")),
                uploaded_image(),
            ],
        );
        let first_messages = [image_turn.clone()];
        let mut first = ing();
        first.messages = &first_messages;
        let mut prev = encode(&first).prefix;
        // 089 录制帧的第 1 轮总 prompt 是 1894，其中图片历史 cache 实际为 1834。
        prev.prompt_tokens = Some(1894);

        let second_messages = [
            image_turn,
            message(
                2,
                Role::Assistant,
                vec![ContentBlock::Text(Arc::from("已读取"))],
            ),
            message(
                3,
                Role::User,
                vec![ContentBlock::Text(Arc::from("再说一次"))],
            ),
        ];
        let mut second = ing();
        second.messages = &second_messages;
        second.prev_prefix = Some(&prev);
        let out = encode(&second);

        assert_eq!(out.drift, None, "图片历史只在尾部追加，不该判漂");
        assert_eq!(
            out.predicted_cache, 1834,
            "已在上一轮前缀内的上传图片必须按 Kimi 实测的视觉块预测"
        );
    }

    #[test]
    fn image_added_only_in_the_current_turn_keeps_block_prediction() {
        let first_messages = [message(
            1,
            Role::User,
            vec![ContentBlock::Text(Arc::from("旧问题"))],
        )];
        let mut first = ing();
        first.messages = &first_messages;
        let mut prev = encode(&first).prefix;
        prev.prompt_tokens = Some(1894);

        let second_messages = [
            first_messages[0].clone(),
            message(
                2,
                Role::Assistant,
                vec![ContentBlock::Text(Arc::from("旧回答"))],
            ),
            message(
                3,
                Role::User,
                vec![ContentBlock::Text(Arc::from("新图")), uploaded_image()],
            ),
        ];
        let mut second = ing();
        second.messages = &second_messages;
        second.prev_prefix = Some(&prev);
        let out = encode(&second);

        assert_eq!(out.drift, None);
        assert_eq!(out.predicted_cache, 1792);
    }
}
