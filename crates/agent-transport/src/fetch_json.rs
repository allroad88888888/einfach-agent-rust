//! 一次非流式 JSON POST 的 `web_sys` 接线：`Client::post_json_async`
//! （issue 125）的连接尝试与响应体读取。跟 [`crate::fetch_request`]（流式
//! POST 的连接尝试）是同一个理由拆出来的同一类文件——「一次请求怎么发、
//! 响应怎么读」是纯 `web_sys`/`js_sys` 接线，跟 `fetch_client.rs` 里
//! 「要不要重试」这类策略决定分开放。
//!
//! 不能直接复用 `fetch_request::attempt_fetch`：headers 不同。流式请求发
//! `Accept: text/event-stream`，这里是一次性 JSON 响应，发
//! `Accept: application/json`，跟 native `client.rs::post_json` 逐字对齐。

use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::RequestInit;

use crate::web_stream_source::describe_js_error;

/// 发一次 JSON POST。返回 `Ok(response)` 涵盖「2xx 成功」与「4xx/5xx 明确
/// 答复」——状态码判断留给调用方，这里只区分「有没有拿到响应」。`Err` 只在
/// 网络层失败时出现（DNS/连接被拒/CORS 拦下），不含状态码，跟
/// `fetch_request::attempt_fetch` 的约定一致。
pub(crate) async fn attempt_json_fetch(
    url: &str,
    api_key: &str,
    body: &[u8],
) -> Result<web_sys::Response, String> {
    let headers = js_sys::Object::new();
    set_header(&headers, "Authorization", &format!("Bearer {api_key}"))?;
    set_header(&headers, "Content-Type", "application/json")?;
    set_header(&headers, "Accept", "application/json")?;

    let body_array = js_sys::Uint8Array::from(body);
    let init = RequestInit::new();
    init.set_method("POST");
    init.set_headers(headers.as_ref());
    init.set_body(&body_array);

    let global = js_sys::global();
    let fetch_fn: js_sys::Function = js_sys::Reflect::get(&global, &JsValue::from_str("fetch"))
        .map_err(|e| describe_js_error(&e))?
        .dyn_into()
        .map_err(|_| "全局作用域上没有 fetch 函数".to_string())?;
    let promise: js_sys::Promise = fetch_fn
        .call2(&global, &JsValue::from_str(url), &init)
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

fn set_header(headers: &js_sys::Object, name: &str, value: &str) -> Result<(), String> {
    js_sys::Reflect::set(
        headers.as_ref(),
        &JsValue::from_str(name),
        &JsValue::from_str(value),
    )
    .map(|_| ())
    .map_err(|e| describe_js_error(&e))
}

/// 把响应体整段读成字符串——`post_json` 是一次性 JSON 响应（不是流），没有
/// `fetch_request::read_bounded_error_body` 那种限长截断的必要性，与 native
/// `post_json` 用 `read_body_limited` 限长的做法有出入，但 issue 125 的验收
/// 范围没有要求对齐这一点，如实记在实做记录里。
pub(crate) async fn read_json_response_body(response: &web_sys::Response) -> String {
    let text_promise = match response.text() {
        Ok(promise) => promise,
        Err(e) => return describe_js_error(&e),
    };
    match JsFuture::from(text_promise).await {
        Ok(value) => value.as_string().unwrap_or_default(),
        Err(e) => describe_js_error(&e),
    }
}
