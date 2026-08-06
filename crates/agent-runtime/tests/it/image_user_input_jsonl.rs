//! 081：图片用户消息经过真 Jsonl 后恢复，字段不能在持久化边界丢失。

use std::sync::Arc;

use agent_core::{AgentId, ContentBlock, Event, Session, SessionConfig, UserImage};
use agent_providers::deepseek::DeepSeek;
use agent_runtime::{open_backend, persist, RunnerCtx, ToolTable};
use agent_tools::ToolExecutor;
use agent_transport::Client;

fn test_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "agent-runtime-image-user-input-{}.jsonl",
        std::process::id()
    ))
}

fn context(path: &std::path::Path) -> RunnerCtx {
    let root = std::env::temp_dir().join(format!(
        "agent-runtime-image-user-input-root-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    RunnerCtx::new(
        Arc::new(DeepSeek),
        Arc::new(Client::new()),
        "https://example.invalid/chat/completions".to_owned(),
        "test-key".to_owned(),
        ToolExecutor::new(root).unwrap(),
        ToolTable::builtin(),
        Vec::new(),
        SessionConfig {
            model: Arc::from("test-model"),
            temperature: None,
            max_tokens: None,
            context_window: None,
        },
        open_backend(Some(path.to_path_buf()), |error| {
            panic!("Jsonl 不应报错：{error}")
        }),
        Box::new(|_| {}),
    )
}

fn assert_restored_image(block: &ContentBlock) {
    match block {
        ContentBlock::Image {
            reference,
            mime,
            name,
        } => {
            assert_eq!(reference.as_ref(), "attachment://img_42");
            assert_eq!(mime.as_ref(), "image/png");
            assert_eq!(name.as_deref(), Some("留存.png"));
        }
        other => panic!("恢复后第二个块应为图片，实际为 {other:?}"),
    }
}

#[test]
fn image_user_input_survives_jsonl_reopen_with_all_fields() {
    let path = test_path();
    let _ = std::fs::remove_file(&path);

    {
        let mut ctx = context(&path);
        let mut session = Session::new(AgentId::root());
        let _ = session.step(Event::UserInput {
            agent: AgentId::root(),
            text: Arc::from("请保存图片"),
            images: vec![UserImage {
                reference: Arc::from("attachment://img_42"),
                mime: Arc::from("image/png"),
                name: Some(Arc::from("留存.png")),
            }],
        });
        persist::sync(&mut ctx, &mut session);
    }

    let backend = open_backend(Some(path.clone()), |error| {
        panic!("加载 Jsonl 不应报错：{error}")
    });
    let recovered = agent_runtime::recover(
        backend.as_ref(),
        AgentId::root(),
        agent_core::DEFAULT_HISTORY_CAP,
        &mut |key| panic!("不该恢复出未知键：{key:?}"),
    )
    .unwrap()
    .expect("写入过图片输入后必须恢复会话");

    let messages = recovered.messages();
    let message = messages.back().expect("恢复后应有用户消息");
    assert_eq!(message.blocks.len(), 2);
    assert_eq!(
        message.blocks[0],
        ContentBlock::Text(Arc::from("请保存图片"))
    );
    assert_restored_image(&message.blocks[1]);
    drop(backend);
    std::fs::remove_file(path).unwrap();
}
