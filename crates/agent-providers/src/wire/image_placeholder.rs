//! 非视觉 provider 的图片降级说明。
//!
//! 外部图片引用可能是上传 URL、任意用户字符串或带签名的地址，不能进入模型上下文。
//! 只有 runtime 生成的内部图片句柄可以安全展示，并提示模型按需调用视觉工具。

const ATTACHMENT_PREFIX: &str = "attachment://";
const IMAGE_HANDLE_PREFIX: &str = "img_";
const VISION_INSPECT_TOOL: &str = "srv:vision/inspect";

/// 将不能由 provider 编码的图片变成确定性的、对模型可见的说明。
///
/// 只有安全 basename 和 `attachment://img_<digits>` 的内部句柄可见；其他字段
/// 既不泄露，也不会把不存在的资源提示给模型。
pub(super) fn dropped_image_placeholder(reference: &str, name: Option<&str>, mime: &str) -> String {
    let text = match name.filter(|name| safe_basename(name)) {
        Some(name) => format!("[用户上传了图片 {name}（{mime}），当前模型看不到图片内容"),
        None => format!("[用户上传了图片（{mime}），当前模型看不到图片内容"),
    };

    match internal_image_handle(reference) {
        Some(handle) => {
            format!("{text}；如需视觉证据，请调用 {VISION_INSPECT_TOOL} 并传入图片句柄 {handle}]")
        }
        None => format!("{text}]"),
    }
}

fn safe_basename(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !name.chars().any(char::is_control)
        && !name.contains('/')
        && !name.contains('\\')
        && !matches!(name, "." | "..")
        && name.as_bytes().get(1) != Some(&b':')
}

fn internal_image_handle(reference: &str) -> Option<&str> {
    let handle = reference.strip_prefix(ATTACHMENT_PREFIX)?;
    let digits = handle.strip_prefix(IMAGE_HANDLE_PREFIX)?;
    (!digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())).then_some(handle)
}

#[cfg(test)]
mod tests {
    use super::dropped_image_placeholder;

    #[test]
    fn exposes_only_a_valid_internal_image_handle() {
        let text =
            dropped_image_placeholder("attachment://img_42", Some("receipt.png"), "image/png");

        assert_eq!(
            text,
            "[用户上传了图片 receipt.png（image/png），当前模型看不到图片内容；如需视觉证据，请调用 srv:vision/inspect 并传入图片句柄 img_42]"
        );
        assert!(!text.contains("attachment://"));
    }

    #[test]
    fn rejects_forged_or_external_references_without_leaking_them() {
        let expected = "[用户上传了图片 receipt.png（image/png），当前模型看不到图片内容]";
        for reference in [
            "attachment://img_",
            "attachment://img_-1",
            "attachment://img_42-extra",
            "attachment://img_42?token=secret",
            "attachment://img_42/next",
            "https://uploads.example.invalid/image.png?signature=secret",
            "ms://opaque-image-reference",
            "untrusted-reference-secret",
        ] {
            let text = dropped_image_placeholder(reference, Some("receipt.png"), "image/png");

            assert_eq!(
                text, expected,
                "reference {reference:?} must use the fallback"
            );
            assert!(!text.contains(reference));
        }
    }

    #[test]
    fn preserves_the_legacy_placeholder_for_untrusted_references() {
        assert_eq!(
            dropped_image_placeholder("https://example.invalid/upload", None, "image/jpeg"),
            "[用户上传了图片（image/jpeg），当前模型看不到图片内容]"
        );
        assert_eq!(
            dropped_image_placeholder("attachment://img_3?bad", Some("photo.jpg"), "image/jpeg"),
            "[用户上传了图片 photo.jpg（image/jpeg），当前模型看不到图片内容]"
        );
    }

    #[test]
    fn preserves_only_safe_basenames() {
        for name in ["photo.png", "截图 2026-08-06.png", "résumé.jpeg"] {
            let text = dropped_image_placeholder("ms://opaque", Some(name), "image/png");
            assert!(text.contains(name), "hid safe basename {name:?}");
        }

        for name in [
            "",
            "/private/NAME_CANARY.png",
            "../NAME_CANARY.png",
            r"C:\private\NAME_CANARY.png",
            r"\\server\share\NAME_CANARY.png",
            "C:NAME_CANARY.png",
            "line\nbreak.png",
            ".",
            "..",
        ] {
            let text = dropped_image_placeholder("ms://opaque", Some(name), "image/png");
            assert_eq!(
                text, "[用户上传了图片（image/png），当前模型看不到图片内容]",
                "exposed unsafe name {name:?}"
            );
            assert!(!text.contains("NAME_CANARY"));
        }

        let oversized = "x".repeat(256);
        let text = dropped_image_placeholder("ms://opaque", Some(&oversized), "image/png");
        assert!(!text.contains(&oversized));
    }
}
