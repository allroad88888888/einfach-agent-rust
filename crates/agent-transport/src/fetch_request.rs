//! 一次 `fetch` 尝试：建请求、发出去、拿到响应后按状态码分流。是
//! `fetch_client.rs` 退避重试循环里「一次连接尝试」的全部细节，拆出来是因为
//! 这一块是纯粹的 `web_sys`/`js_sys` 接线，跟「要不要重试、退避多久」这类
//! 策略决定（留在 `fetch_client.rs`）是两件事。

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{AbortController, ReadableStreamDefaultReader, RequestInit};

use crate::web_stream_source::{WebStreamSource, describe_js_error};

/// 错误响应体最多读这么多字节——与 native `client.rs` 的
/// `MAX_ERROR_BODY` 同一个数字，但读取方式不同（`fetch` 没有「只读 N 字节
/// 就停」的低层句柄，这里是整段 `Response::text()` 读完之后按字节数截断）。
const MAX_ERROR_BODY: usize = 8 * 1024;

/// 发起一次 `fetch`。返回 `Ok(response)` 涵盖「2xx 成功」与「4xx/5xx 明确
/// 答复」——跟 native `try_connect` 一样，状态码判断留给调用方，这里只区分
/// 「有没有拿到响应」。`Err` 只在网络层失败时出现（DNS/连接被拒/CORS 拦下/
/// 主动 abort），不含状态码——`fetch` 不区分「建连接失败」和「等状态行
/// 超时」，两者在浏览器里都是同一种 Promise rejection，这是与 native
/// 023 那套精细分类的一处平台性差异，如实记在 113 的实做记录里。
pub(crate) async fn attempt_fetch(
    url: &str,
    api_key: &str,
    body: &[u8],
    controller: &AbortController,
) -> Result<web_sys::Response, String> {
    let init = build_request_init(api_key, body, controller).map_err(|e| describe_js_error(&e))?;
    let response_value = call_global_fetch(url, &init)
        .await
        .map_err(|e| describe_js_error(&e))?;
    response_value
        .dyn_into::<web_sys::Response>()
        .map_err(|_| "fetch 返回了非 Response 对象".to_string())
}

fn build_request_init(
    api_key: &str,
    body: &[u8],
    controller: &AbortController,
) -> Result<RequestInit, JsValue> {
    let headers = js_sys::Object::new();
    set_header(&headers, "Authorization", &format!("Bearer {api_key}"))?;
    set_header(&headers, "Content-Type", "application/json")?;
    set_header(&headers, "Accept", "text/event-stream")?;

    let body_array = js_sys::Uint8Array::from(body);
    let init = RequestInit::new();
    init.set_method("POST");
    init.set_headers(headers.as_ref());
    init.set_body(&body_array);
    init.set_signal(Some(&controller.signal()));

    Ok(init)
}

fn set_header(headers: &js_sys::Object, name: &str, value: &str) -> Result<(), JsValue> {
    js_sys::Reflect::set(
        headers.as_ref(),
        &JsValue::from_str(name),
        &JsValue::from_str(value),
    )
    .map(|_| ())
}

/// 全局 `fetch`，不经 `web_sys::window()`——同一份代码要能在主线程、
/// Worker（114 大概率会用的形态，见 `fetch_client.rs` 顶部关于同步阻塞的
/// 讨论）、以及本 crate 自己用 Node 跑的 wasm 测试里都拿到 `fetch`，三者
/// 共同点只有「全局作用域上有一个 `fetch` 函数」，所以直接反射取。
async fn call_global_fetch(url: &str, init: &RequestInit) -> Result<JsValue, JsValue> {
    let global = js_sys::global();
    let fetch_fn: js_sys::Function = js_sys::Reflect::get(&global, &JsValue::from_str("fetch"))?
        .dyn_into()
        .map_err(|_| JsValue::from_str("全局作用域上没有 fetch 函数"))?;
    let promise: js_sys::Promise = fetch_fn
        .call2(&global, &JsValue::from_str(url), init)?
        .dyn_into()?;
    JsFuture::from(promise).await
}

/// 把响应体包成 [`WebStreamSource`]（`ChunkSource` 的生产实现）。`cancel`/
/// `poll_interval` 转给它用来在「卡在一次读中间」时也能观察到取消，见
/// `web_stream_source.rs` 模块文档。
pub(crate) fn response_stream_source<'a>(
    response: web_sys::Response,
    controller: AbortController,
    cancel: &'a AtomicBool,
    poll_interval: Duration,
) -> Result<WebStreamSource<'a>, String> {
    let stream = response
        .body()
        .ok_or_else(|| "响应没有可读的 body".to_string())?;
    let reader: ReadableStreamDefaultReader = stream
        .get_reader()
        .dyn_into()
        .map_err(|_| "get_reader() 返回了非预期类型".to_string())?;
    Ok(WebStreamSource::new(
        reader,
        controller,
        cancel,
        poll_interval,
    ))
}

/// 对应 native `client.rs` 的 `read_bounded`：非 200 响应体限长读取，不把
/// 整个畸形响应吃进内存。`fetch` 没有「读 N 字节就停」的底层句柄，这里退化
/// 成「整段读完再按字节截断」——错误响应体在实践中远小于这个上限（三家的
/// JSON 错误体通常几百字节，见 PROVIDERS.md §四），这个差异不影响
/// `TransportError::Http { status, body }` 在正常场景下的取值。
pub(crate) async fn read_bounded_error_body(response: &web_sys::Response) -> String {
    let text_promise = match response.text() {
        Ok(p) => p,
        Err(e) => return describe_js_error(&e),
    };
    match JsFuture::from(text_promise).await {
        Ok(value) => {
            let text = value.as_string().unwrap_or_default();
            truncate_to_byte_limit(&text, MAX_ERROR_BODY)
        }
        Err(e) => describe_js_error(&e),
    }
}

fn truncate_to_byte_limit(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}
