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

/// 埋了四位数字的测试图，返回 `(那四位数, data URL)`。
///
/// 数字由 `nonce` 派生，于是同一次运行内**固定**（多模态那组的几个观测必须看
/// 同一张图，否则比不了缓存），换一次运行就换一张（冷缓存开始，跟 [`system`]
/// 把 nonce 放最前面是同一个道理）。
///
/// 为什么是「图里印数字」而不是随便一张图：探针要判的是模型**看没看见**，
/// 不是 API 收没收。只有图里有一个它没处猜的东西，这条才是行为级断言。
pub fn image_digits(nonce: &str) -> (String, String) {
    image_digits_scaled(nonce, DEFAULT_SCALE)
}

/// 默认放大倍数。10 倍时数字 50×70 像素，整图 270×110——肉眼和模型都认得出。
pub const DEFAULT_SCALE: usize = 10;

/// 同一个数字、换一个放大倍数的同一张图。
///
/// 存在理由是**计价**：默认那张只有 270×110，量出来的 token 数不能外推到用户
/// 真会上传的照片上。换两个尺寸再打一次，才知道那个数字是固定开销还是随面积长。
pub fn image_digits_scaled(nonce: &str, scale: usize) -> (String, String) {
    let (digits, png) = image_png_scaled(nonce, scale);
    (digits, format!("data:image/png;base64,{}", crate::b64::encode(&png)))
}

/// 同一张图的 PNG 原始字节，返回 `(那四位数, 字节)`。
///
/// 单独暴露是给 `bin/multimodal --dump` 用的：手写的 PNG 编码器只被自己的单测
/// 验过算术，**没被真解码器验过**。图要是坏的，探针会报「模型没看见」，而那跟
/// 「模型不支持」长得一模一样——两者必须能分开，所以得能把它落成文件给别的
/// 解码器看。
pub fn image_png(nonce: &str) -> (String, Vec<u8>) {
    image_png_scaled(nonce, DEFAULT_SCALE)
}

/// 见 [`image_digits_scaled`]。留白按倍数一起放大，免得大图挤在边框上。
pub fn image_png_scaled(nonce: &str, scale: usize) -> (String, Vec<u8>) {
    let digits = format!("{}", 1000 + fnv1a(nonce) % 9000);
    let bmp = crate::digits::render(&digits, scale, scale * 2);
    (digits, crate::png::encode_gray(bmp.width, bmp.height, &bmp.pixels))
}

/// FNV-1a。要的只是「由 nonce 确定性地散出一个数」，不要密码学强度；
/// 用标准库的 `DefaultHasher` 反而不行——它的取值**不保证跨版本稳定**，
/// 而这里的素材必须逐字节可复现。
fn fnv1a(s: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
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
