//! 从持久化 JSONL 中找出 core 已保存的内部图片句柄。
//!
//! 这不是恢复会话状态的第二套实现：actor 仍由 `agent_runtime::recover` 恢复。
//! 此处只在 actor 启动前把历史 `ContentBlock::Image` 的内部引用标记为没有字节的
//! attachment tombstone，供后续 facade 返回稳定的 unavailable 语义。

use std::path::Path;

use serde_json::Value;

use crate::attachments::ImageHandle;

pub(crate) fn recovered_handles(path: Option<&Path>) -> Vec<ImageHandle> {
    let Some(path) = path else {
        return Vec::new();
    };
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut handles = Vec::new();
    for line in content.lines() {
        if let Ok(value) = serde_json::from_str(line) {
            collect_handles(&value, &mut handles);
        }
    }
    handles
}

fn collect_handles(value: &Value, handles: &mut Vec<ImageHandle>) {
    match value {
        Value::Array(values) => values
            .iter()
            .for_each(|value| collect_handles(value, handles)),
        Value::Object(values) => {
            if let Some(Value::Object(image)) = values.get("Image")
                && let Some(Value::String(reference)) = image.get("reference")
                && let Some(handle) = reference.strip_prefix("attachment://")
                && let Some(handle) = ImageHandle::parse(handle)
            {
                handles.push(handle);
            }
            values
                .values()
                .for_each(|value| collect_handles(value, handles));
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::recovered_handles;

    #[test]
    fn extracts_only_internal_image_blocks() {
        let path = std::env::temp_dir().join(format!(
            "agent-server-attachment-recovery-{}-{}.jsonl",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(
            &path,
            r#"{"Image":{"reference":"attachment://img_42"}}
{"Text":"attachment://img_7"}
{"Image":{"reference":"https://example.invalid/image"}}"#,
        )
        .unwrap();

        let handles = recovered_handles(Some(&path));
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            handles
                .iter()
                .map(|handle| handle.as_str())
                .collect::<Vec<_>>(),
            vec!["img_42"]
        );
    }
}
