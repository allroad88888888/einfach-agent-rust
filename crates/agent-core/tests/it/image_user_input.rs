//! 081：图片跟随用户输入进入历史；文本顺序、undo/redo 与协议闸都在这里验收。

use std::sync::Arc;

use agent_core::{
    AgentId, ContentBlock, Effect, Event, Notice, Session, TurnStatus, UndoReport, UserImage,
};

fn image(reference: &str, mime: &str, name: Option<&str>) -> UserImage {
    UserImage {
        reference: Arc::from(reference),
        mime: Arc::from(mime),
        name: name.map(Arc::from),
    }
}

fn input(text: &str, images: Vec<UserImage>) -> Event {
    Event::UserInput {
        agent: AgentId::root(),
        text: Arc::from(text),
        images,
    }
}

fn only_message_blocks(session: &Session) -> Vec<ContentBlock> {
    assert_eq!(session.messages().len(), 1, "这一轮应恰好写入一条用户消息");
    session
        .messages()
        .back()
        .expect("用户消息应存在")
        .blocks
        .clone()
}

fn assert_image(block: &ContentBlock, reference: &str, mime: &str, name: Option<&str>) {
    match block {
        ContentBlock::Image {
            reference: actual_reference,
            mime: actual_mime,
            name: actual_name,
        } => {
            assert_eq!(
                actual_reference.as_ref(),
                reference,
                "reference 必须逐字符保留"
            );
            assert_eq!(actual_mime.as_ref(), mime, "mime 必须逐字符保留");
            assert_eq!(actual_name.as_deref(), name, "name 必须逐字符保留");
        }
        other => panic!("期待 Image，实际为 {other:?}"),
    }
}

#[test]
fn text_only_input_keeps_the_old_single_text_block() {
    let mut session = Session::new(AgentId::root());

    let effects = session.step(input("北京天气", Vec::new()));

    assert!(matches!(effects.last(), Some(Effect::CallProvider { .. })));
    assert_eq!(
        only_message_blocks(&session),
        vec![ContentBlock::Text(Arc::from("北京天气"))],
        "无图片必须仍是旧路径的 vec![ContentBlock::Text(text)]"
    );
}

#[test]
fn two_input_images_follow_text_and_preserve_host_order() {
    let mut session = Session::new(AgentId::root());
    let first = image("ms://first", "image/png", Some("first.png"));
    let second = image("ms://second", "image/jpeg", None);

    let _ = session.step(input("请看附件", vec![first, second]));

    let blocks = only_message_blocks(&session);
    assert_eq!(blocks.len(), 3, "文本与两张图各占一个并列块");
    assert_eq!(blocks[0], ContentBlock::Text(Arc::from("请看附件")));
    assert_image(&blocks[1], "ms://first", "image/png", Some("first.png"));
    assert_image(&blocks[2], "ms://second", "image/jpeg", None);
}

#[test]
fn image_block_bytes_are_stable_and_text_first() {
    let images = vec![
        image("ms://first", "image/png", Some("first.png")),
        image("ms://second", "image/jpeg", None),
    ];
    let expected = vec![
        ContentBlock::Text(Arc::from("请按顺序看图")),
        ContentBlock::Image {
            reference: Arc::from("ms://first"),
            mime: Arc::from("image/png"),
            name: Some(Arc::from("first.png")),
        },
        ContentBlock::Image {
            reference: Arc::from("ms://second"),
            mime: Arc::from("image/jpeg"),
            name: None,
        },
    ];
    let mut left = Session::new(AgentId::root());
    let mut right = Session::new(AgentId::root());

    let _ = left.step(input("请按顺序看图", images.clone()));
    let _ = right.step(input("请按顺序看图", images));

    let left_bytes = serde_json::to_vec(&only_message_blocks(&left)).unwrap();
    let expected_bytes = serde_json::to_vec(&expected).unwrap();
    assert_eq!(left_bytes, expected_bytes, "文本块必须在图片块之前");
    assert_eq!(
        left_bytes,
        serde_json::to_vec(&only_message_blocks(&right)).unwrap(),
        "相同输入的块序列化字节必须稳定"
    );
}

#[test]
fn undo_then_redo_restores_every_image_field() {
    let mut session = Session::new(AgentId::root());
    let _ = session.step(input(
        "可撤销图片",
        vec![image("ms://invoice", "image/png", Some("发票.png"))],
    ));

    assert!(matches!(session.undo_turn(), UndoReport::Applied { .. }));
    assert!(
        session.messages().is_empty(),
        "undo 后图片消息必须从历史消失"
    );
    assert!(matches!(session.redo_turn(), UndoReport::Applied { .. }));

    let blocks = only_message_blocks(&session);
    assert_eq!(blocks[0], ContentBlock::Text(Arc::from("可撤销图片")));
    assert_image(&blocks[1], "ms://invoice", "image/png", Some("发票.png"));
}

#[test]
fn image_input_outside_idle_is_still_a_protocol_violation() {
    let mut session = Session::new(AgentId::root());
    let _ = session.step(input("先进入 Thinking", Vec::new()));
    let history_len = session.history_len();

    let effects = session.step(input(
        "不该被收下",
        vec![image("ms://late", "image/png", None)],
    ));

    assert!(matches!(
        effects.as_slice(),
        [Effect::Emit(Notice::ProtocolViolation {
            state: TurnStatus::Thinking,
            ..
        })]
    ));
    assert_eq!(
        session.history_len(),
        history_len,
        "协议违规不能写进 undo 历史"
    );
    assert_eq!(
        session.messages().len(),
        1,
        "协议违规不能偷偷加一条图片消息"
    );
}
