//! HTTP 图片输入的批次配额与 JSON 请求体预算。

use agent_transport::{MAX_IMAGE_BYTES, UploadError};

use crate::http::error::ApiError;

/// 一轮输入最多接收的图片数。provider 可以支持更多，但 HTTP 入口必须有 DoS 边界。
const MAX_INPUT_IMAGES: usize = 8;

/// 一轮所有图片的原始字节总量不超过当前单图上传上限。
const MAX_INPUT_IMAGE_BYTES: usize = MAX_IMAGE_BYTES;

/// JSON 数组里的一个 u8 最坏编码为 `255,`，需要四个字节。
const JSON_BYTE_WORST_CASE_EXPANSION: usize = 4;

/// 为文本、文件名、MIME 和 JSON 结构保留的额外空间。
const INPUT_METADATA_BUDGET_BYTES: usize = 1024 * 1024;

/// 仅挂在 `/sessions/{id}/input`；不是全局放开，也不是无限制。
pub(super) const INPUT_BODY_LIMIT_BYTES: usize =
    MAX_INPUT_IMAGE_BYTES * JSON_BYTE_WORST_CASE_EXPANSION + INPUT_METADATA_BUDGET_BYTES;

/// 在任何上传发生前验证整批图片的数量、单图大小与累计原始字节数。
pub(super) fn validate_image_quota(
    image_sizes: impl IntoIterator<Item = usize>,
) -> Result<(), ApiError> {
    let mut image_count = 0usize;
    let mut total_bytes = 0usize;

    for image_bytes in image_sizes {
        image_count += 1;
        if image_count > MAX_INPUT_IMAGES {
            return Err(ApiError::bad_request(format!(
                "每轮最多上传 {MAX_INPUT_IMAGES} 张图片"
            )));
        }
        if image_bytes > MAX_IMAGE_BYTES {
            return Err(ApiError::bad_request(
                UploadError::TooLarge {
                    actual_bytes: image_bytes,
                    limit_bytes: MAX_IMAGE_BYTES,
                }
                .to_string(),
            ));
        }
        total_bytes = total_bytes
            .checked_add(image_bytes)
            .ok_or_else(|| ApiError::bad_request("图片累计大小超过平台可处理范围"))?;
        if total_bytes > MAX_INPUT_IMAGE_BYTES {
            return Err(ApiError::bad_request(format!(
                "图片累计大小超过限制：{total_bytes} bytes（上限 {MAX_INPUT_IMAGE_BYTES} bytes）"
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ninth_image_is_rejected() {
        let error = validate_image_quota([0; MAX_INPUT_IMAGES + 1])
            .expect_err("第九张图片必须在 HTTP 边界被拒绝");

        assert!(
            format!("{error:?}").contains("最多上传 8 张图片"),
            "数量错误必须可读：{error:?}"
        );
    }

    #[test]
    fn aggregate_raw_bytes_over_the_single_image_limit_are_rejected() {
        let first = MAX_INPUT_IMAGE_BYTES / 2 + 1;
        let second = MAX_INPUT_IMAGE_BYTES - first + 1;
        let error =
            validate_image_quota([first, second]).expect_err("累计原始字节超过 100 MiB 必须被拒绝");

        assert!(
            format!("{error:?}").contains("图片累计大小超过限制"),
            "累计大小错误必须可读：{error:?}"
        );
    }
}
