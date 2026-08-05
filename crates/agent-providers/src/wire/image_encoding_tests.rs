//! 082：共享 wire 的图片数组形状与纯文本兼容性。

use std::sync::Arc;

use agent_core::{ContentBlock, Message, MessageId, Role};
use serde_json::{Value, json};

use super::messages::history_with_image_support;

fn message(id: u64, blocks: Vec<ContentBlock>) -> Message {
    Message {
        id: MessageId(id),
        role: Role::User,
        blocks,
    }
}

fn image(reference: &str, mime: &str, name: Option<&str>) -> ContentBlock {
    ContentBlock::Image {
        reference: Arc::from(reference),
        mime: Arc::from(mime),
        name: name.map(Arc::from),
    }
}

#[test]
fn text_only_wire_content_remains_exactly_string_even_when_images_are_supported() {
    let messages = [message(1, vec![ContentBlock::Text(Arc::from("北京天气"))])];

    let encoded = history_with_image_support(&messages, true);

    assert_eq!(encoded.dropped_images, 0);
    assert_eq!(
        encoded.messages,
        vec![json!({"role": "user", "content": "北京天气"})],
        "无图消息必须保持既有字符串 content，不能改成数组"
    );
    assert_eq!(
        serde_json::to_vec(&encoded.messages).unwrap(),
        serde_json::to_vec(&vec![json!({"role": "user", "content": "北京天气"})]).unwrap(),
        "无图消息必须逐字节保持既有字符串 content"
    );
}

#[test]
fn image_wire_content_is_text_then_all_images_in_block_order() {
    let messages = [
        message(1, vec![ContentBlock::Text(Arc::from("仍是纯文本"))]),
        message(
            2,
            vec![
                ContentBlock::Text(Arc::from("请看附件")),
                image("ms://first", "image/png", Some("first.png")),
                image("ms://second", "image/jpeg", None),
            ],
        ),
    ];

    let first = history_with_image_support(&messages, true);
    let second = history_with_image_support(&messages, true);

    assert_eq!(first.dropped_images, 0);
    assert_eq!(
        first.messages[0],
        json!({"role": "user", "content": "仍是纯文本"}),
        "同一历史里的纯文本消息也不能变成数组"
    );
    assert_eq!(
        first.messages[1]["content"],
        json!([
            {"type": "text", "text": "请看附件"},
            {"type": "image_url", "image_url": {"url": "ms://first"}},
            {"type": "image_url", "image_url": {"url": "ms://second"}},
        ]),
        "图片引用必须原样写入，且文本块始终在图片块之前"
    );
    assert!(matches!(first.messages[1]["content"], Value::Array(_)));
    assert_eq!(
        serde_json::to_vec(&first.messages).unwrap(),
        serde_json::to_vec(&second.messages).unwrap(),
        "同一带图消息两次编码的 JSON 字节必须相同"
    );
}
