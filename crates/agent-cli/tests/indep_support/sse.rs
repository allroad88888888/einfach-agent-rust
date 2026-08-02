//! 构造 deepseek 形状的流式帧（`probes/PROVIDERS.md` §三：`data: {json}` +
//! 末尾 `data: [DONE]`，工具参数按 `index` 累加）。
//!
//! `srv:shell/exec` 在真实请求体里被编码成 `srv_3Ashell_2Fexec`（`:` → `_3A`，
//! `/` → `_2F`）——这是黑盒探测真二进制时从假服务器收到的实际请求体里现读出来的
//! 事实，脚本化的 `tool_calls` 响应必须用这个编码后的名字模型才认得出来。

pub const SHELL_EXEC_WIRE_NAME: &str = "srv_3Ashell_2Fexec";

/// 一段纯文本回复：一帧 `content` + `finish_reason: stop`，随后 `[DONE]`。
pub fn text_reply(text: &str) -> String {
    let content = serde_json::to_string(text).expect("json string");
    format!("data: {{\"choices\":[{{\"delta\":{{\"content\":{content}}},\"finish_reason\":\"stop\"}}]}}\n\ndata: [DONE]\n\n")
}

/// 一次工具调用：三帧（角色+函数名开头、参数追加、`finish_reason: tool_calls`）
/// 再加 `[DONE]`，跟 `probes/PROVIDERS.md` 描述的“工具参数按 index 累加”一致。
pub fn tool_call(call_id: &str, wire_name: &str, args_json: &str) -> String {
    let args = serde_json::to_string(args_json).expect("json string");
    format!(
        "data: {{\"choices\":[{{\"delta\":{{\"role\":\"assistant\",\"tool_calls\":[{{\"index\":0,\"id\":\"{call_id}\",\"type\":\"function\",\"function\":{{\"name\":\"{wire_name}\",\"arguments\":\"\"}}}}]}},\"finish_reason\":null}}]}}\n\n\
         data: {{\"choices\":[{{\"delta\":{{\"tool_calls\":[{{\"index\":0,\"function\":{{\"arguments\":{args}}}}}]}},\"finish_reason\":null}}]}}\n\n\
         data: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"tool_calls\"}}]}}\n\n\
         data: [DONE]\n\n"
    )
}
