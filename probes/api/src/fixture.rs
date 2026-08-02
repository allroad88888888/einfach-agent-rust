//! 请求素材。所有内容都由 `nonce` + 参数**确定性**生成 —— 前缀缓存靠逐字节相等，
//! 素材里混进任何不确定的东西，整个探针就没有可比性了。

use serde_json::{Value, json};

/// system prompt。`nonce` 放在**最前面**：换 nonce 等于整条前缀作废，
/// 于是每次运行都从冷缓存开始，不会读到上一轮留下的缓存。
pub fn system(nonce: &str, records: usize) -> String {
    let mut s = format!(
        "[run:{nonce}] You are a precise assistant working through a fixed reference \
         document. Answer only with the requested identifier. Do not explain.\n\n\
         === REFERENCE DOCUMENT (do not summarize) ===\n"
    );
    for i in 0..records {
        s.push_str(&format!(
            "Record {i:04}: region=R{}, tier={}, quota={}, status=active, owner=team-{}\n",
            i % 7,
            i % 4,
            1000 + i * 13,
            i % 11
        ));
    }
    s.push_str("=== END REFERENCE DOCUMENT ===\n");
    s
}

fn tool(name: &str, desc: &str) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": desc,
            "parameters": {
                "type": "object",
                "properties": { "query": { "type": "string", "description": "查询串" } },
                "required": ["query"]
            }
        }
    })
}

pub fn tool_a() -> Value {
    tool("lookup_record", "按编号查参考文档里的一条记录")
}

pub fn tool_b() -> Value {
    tool("summarize_region", "汇总某个 region 下所有记录的配额")
}

pub const ASK: &str = "Record 0042 的 owner 是谁？只答 team-N。";

pub fn messages(nonce: &str, records: usize, ask: &str) -> Value {
    json!([
        { "role": "system", "content": system(nonce, records) },
        { "role": "user",   "content": ask }
    ])
}

/// 多轮对话的**前段**，不含末尾提问 —— 调用方自己接尾巴，这样「末尾追加」
/// 和「中间改写」两个对照才是严格同源的。
///
/// `mutate_at` 不为 None 时改写那一轮的用户消息，长度刻意接近原文，
/// 模拟一次压缩重写：改的是中段，不是尾巴。
pub fn conversation(nonce: &str, records: usize, turns: usize, mutate_at: Option<usize>) -> Vec<Value> {
    let mut msgs = vec![json!({ "role": "system", "content": system(nonce, records) })];
    for t in 0..turns {
        let body = if Some(t) == mutate_at {
            format!("第 {t} 轮：【压缩后】该轮已被摘要替换，涉及 Record {:04}。", t * 7)
        } else {
            format!("第 {t} 轮：请确认 Record {:04} 的 tier。", t * 7)
        };
        msgs.push(json!({ "role": "user", "content": body }));
        msgs.push(json!({
            "role": "assistant",
            "content": format!("第 {t} 轮回答：tier={}。", t % 4)
        }));
    }
    msgs
}

pub fn user(text: &str) -> Value {
    json!({ "role": "user", "content": text })
}

pub fn assistant(text: &str) -> Value {
    json!({ "role": "assistant", "content": text })
}

/// 请求骨架。`max_tokens` 压到最小以省输出费。
pub fn body(provider: &str, model: &str, messages: Value, tools: Vec<Value>) -> Value {
    json!({
        "model": model,
        "messages": messages,
        "tools": tools,
        "max_tokens": 16,
        "temperature": crate::caps::temperature(provider),
        "stream": false
    })
}
