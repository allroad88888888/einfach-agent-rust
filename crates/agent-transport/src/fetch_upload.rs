//! `upload.rs` 的 wasm 对应物：图片上传换成 `fetch`，**语义不动**（issue 113
//! 范围提示：M11 的东西，只做平台适配）。multipart 编码、boundary 选取、
//! 响应体解析全部复用 `upload.rs` 的 `pub(crate)` helper，这个文件只有
//! 「怎么把 multipart body 通过 `fetch` 发出去、怎么把 `Response` 翻译成
//! `UploadError`」这一件事。

use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Headers, RequestInit};

use crate::upload::{
    ImageUpload, MAX_IMAGE_BYTES, UploadError, UploadResponse, boundary_for, multipart_body,
    redact, upload_response_reference,
};
use crate::web_stream_source::describe_js_error;

pub(crate) async fn send(
    base_url: &str,
    api_key: &str,
    image: ImageUpload<'_>,
) -> Result<String, UploadError> {
    if image.bytes.len() > MAX_IMAGE_BYTES {
        return Err(UploadError::TooLarge {
            actual_bytes: image.bytes.len(),
            limit_bytes: MAX_IMAGE_BYTES,
        });
    }

    let boundary = boundary_for(image.bytes);
    let body = multipart_body(&boundary, image);
    if body.len() > MAX_IMAGE_BYTES {
        return Err(UploadError::TooLarge {
            actual_bytes: body.len(),
            limit_bytes: MAX_IMAGE_BYTES,
        });
    }

    let url = format!("{}/files", base_url.trim_end_matches('/'));
    let response = match do_fetch(&url, api_key, &boundary, &body).await {
        Ok(response) => response,
        Err(message) => {
            return Err(UploadError::Network {
                message: redact(&message, api_key),
            });
        }
    };

    match response.status() {
        401 => Err(UploadError::Unauthorized),
        status if status >= 400 => Err(UploadError::ProviderRejected { status }),
        _ => response_reference(response, api_key).await,
    }
}

async fn do_fetch(
    url: &str,
    api_key: &str,
    boundary: &str,
    body: &[u8],
) -> Result<web_sys::Response, String> {
    let headers = Headers::new().map_err(|e| describe_js_error(&e))?;
    headers
        .set("Authorization", &format!("Bearer {api_key}"))
        .map_err(|e| describe_js_error(&e))?;
    headers
        .set(
            "Content-Type",
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .map_err(|e| describe_js_error(&e))?;
    headers
        .set("Accept", "application/json")
        .map_err(|e| describe_js_error(&e))?;

    let body_array = js_sys::Uint8Array::from(body);
    let init = RequestInit::new();
    init.set_method("POST");
    init.set_headers(&headers);
    init.set_body(&body_array);

    let request =
        web_sys::Request::new_with_str_and_init(url, &init).map_err(|e| describe_js_error(&e))?;

    let global = js_sys::global();
    let fetch_fn: js_sys::Function = js_sys::Reflect::get(&global, &JsValue::from_str("fetch"))
        .map_err(|e| describe_js_error(&e))?
        .dyn_into()
        .map_err(|_| "全局作用域上没有 fetch 函数".to_string())?;
    let promise: js_sys::Promise = fetch_fn
        .call1(&global, &request)
        .map_err(|e| describe_js_error(&e))?
        .dyn_into()
        .map_err(|_| "fetch 没有返回 Promise".to_string())?;
    let response_value = JsFuture::from(promise)
        .await
        .map_err(|e| describe_js_error(&e))?;
    response_value
        .dyn_into::<web_sys::Response>()
        .map_err(|_| "fetch 返回了非 Response 对象".to_string())
}

async fn response_reference(
    response: web_sys::Response,
    api_key: &str,
) -> Result<String, UploadError> {
    let text_promise = response.text().map_err(|e| UploadError::Network {
        message: redact(&describe_js_error(&e), api_key),
    })?;
    let text_value = JsFuture::from(text_promise)
        .await
        .map_err(|e| UploadError::Network {
            message: redact(&describe_js_error(&e), api_key),
        })?;
    let body = text_value.as_string().unwrap_or_default();
    let parsed: UploadResponse =
        serde_json::from_str(&body).map_err(|_| UploadError::InvalidResponse {
            message: "缺少合法 JSON 文件 id".to_string(),
        })?;
    upload_response_reference(parsed)
}
