//! 083：三家 adapter 对图片能力的显式接线与可见降级。

use std::sync::Arc;

use agent_core::{
    Adjustment, ContentBlock, Message, MessageId, RequestIntent, Role, SessionConfig,
};
use agent_providers::{Ingredients, Provider, deepseek::DeepSeek, glm::Glm, kimi::Kimi};
use serde_json::{Value, json};

fn config() -> SessionConfig {
    SessionConfig {
        model: Arc::from("image-test"),
        temperature: None,
        max_tokens: None,
        context_window: None,
    }
}

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

fn encode<P: Provider + ?Sized>(provider: &P, messages: &[Message]) -> agent_providers::Encoded {
    let config = config();
    provider.encode(&Ingredients {
        system: &[],
        messages,
        tools: &[],
        late_tools: &[],
        late_system: &[],
        config: &config,
        intent: RequestIntent::Free,
        prev_prefix: None,
    })
}

fn body<P: Provider + ?Sized>(
    provider: &P,
    messages: &[Message],
) -> (Vec<u8>, Value, Vec<Adjustment>) {
    let encoded = encode(provider, messages);
    let body = serde_json::from_slice(&encoded.body).expect("adapter body must be JSON");
    (encoded.body, body, encoded.adjustments)
}

fn image_messages() -> Vec<Message> {
    vec![message(
        1,
        vec![
            ContentBlock::Text(Arc::from("请检查两份附件")),
            image("ms://receipt-opaque", "image/png", Some("发票.png")),
            image("ms://unnamed-opaque", "image/jpeg", None),
        ],
    )]
}

#[test]
fn kimi_encodes_two_opaque_images_without_a_drop_adjustment() {
    let messages = image_messages();
    let (_, body, adjustments) = body(&Kimi, &messages);

    assert_eq!(
        body["messages"],
        json!([
            {"role": "user", "content": [
                {"type": "text", "text": "请检查两份附件"},
                {"type": "image_url", "image_url": {"url": "ms://receipt-opaque"}},
                {"type": "image_url", "image_url": {"url": "ms://unnamed-opaque"}},
            ]}
        ])
    );
    assert!(
        !adjustments
            .iter()
            .any(|adjustment| matches!(adjustment, Adjustment::ImagesDropped { .. })),
        "Kimi 实际收到图片时不得误报 ImagesDropped"
    );
}

#[test]
fn deepseek_and_glm_hide_refs_and_report_every_dropped_image() {
    let messages = image_messages();
    let expected = "请检查两份附件\n[用户上传了图片 发票.png（image/png），当前模型看不到图片内容]\n[用户上传了图片（image/jpeg），当前模型看不到图片内容]";

    for (name, (bytes, body, adjustments)) in [
        ("deepseek", body(&DeepSeek, &messages)),
        ("glm", body(&Glm, &messages)),
    ] {
        let wire = String::from_utf8(bytes).expect("adapter body is UTF-8 JSON");
        assert!(
            !wire.contains("image_url")
                && !wire.contains("ms://receipt-opaque")
                && !wire.contains("ms://unnamed-opaque"),
            "{name} 不得把 image_url 或不透明引用漏进请求"
        );
        assert_eq!(body["messages"][0]["content"], json!(expected));
        assert_eq!(
            adjustments,
            vec![Adjustment::ImagesDropped { count: 2 }],
            "{name} 降级两张图片必须精确报告两张"
        );
    }
}

#[test]
fn deepseek_and_glm_hide_path_shaped_image_names() {
    for unsafe_name in [
        "/private/POSIX_NAME_CANARY.png",
        "../TRAVERSAL_NAME_CANARY.png",
        r"C:\private\WINDOWS_NAME_CANARY.png",
        r"\\server\share\UNC_NAME_CANARY.png",
    ] {
        let messages = [message(
            1,
            vec![image("attachment://img_7", "image/png", Some(unsafe_name))],
        )];
        let expected = "[用户上传了图片（image/png），当前模型看不到图片内容；如需视觉证据，请调用 srv:vision/inspect 并传入图片句柄 img_7]";

        for (provider_name, (_, body, adjustments)) in [
            ("deepseek", body(&DeepSeek, &messages)),
            ("glm", body(&Glm, &messages)),
        ] {
            assert_eq!(body["messages"][0]["content"], json!(expected));
            assert_eq!(adjustments, vec![Adjustment::ImagesDropped { count: 1 }]);
            assert!(
                !body.to_string().contains(unsafe_name),
                "{provider_name} exposed unsafe image name {unsafe_name:?}"
            );
        }
    }
}

#[test]
fn fallback_placeholder_is_deterministic_and_independent_of_history_position() {
    let photo = image("ms://same-photo", "image/png", None);
    let early = [message(
        1,
        vec![ContentBlock::Text(Arc::from("附件")), photo.clone()],
    )];
    let later = [
        message(1, vec![ContentBlock::Text(Arc::from("前一条历史"))]),
        message(2, vec![ContentBlock::Text(Arc::from("附件")), photo]),
    ];

    let (first_bytes, first_body, _) = body(&DeepSeek, &early);
    let (second_bytes, _, _) = body(&DeepSeek, &early);
    let (_, later_body, _) = body(&DeepSeek, &later);

    assert_eq!(
        first_bytes, second_bytes,
        "同一图片重复编码的 bytes 必须相同"
    );
    assert_eq!(
        first_body["messages"][0]["content"], later_body["messages"][1]["content"],
        "占位文本只能由图片块字段决定，不能依赖历史位置"
    );
}

#[test]
fn text_only_bodies_remain_strings_for_all_providers() {
    let messages = [message(1, vec![ContentBlock::Text(Arc::from("北京天气"))])];

    for (name, provider) in [
        ("kimi", &Kimi as &dyn Provider),
        ("deepseek", &DeepSeek as &dyn Provider),
        ("glm", &Glm as &dyn Provider),
    ] {
        let (first_bytes, first_body, adjustments) = body(provider, &messages);
        let (second_bytes, _, _) = body(provider, &messages);
        assert_eq!(
            first_body["messages"][0],
            json!({"role": "user", "content": "北京天气"}),
            "{name} 的无图会话必须逐字节保持 082 之后的字符串形状"
        );
        assert_eq!(first_bytes, second_bytes, "{name} 的无图编码必须确定");
        assert!(adjustments.is_empty(), "{name} 无图时不应产生新调整");
    }
}
