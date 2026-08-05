//! 063 的共用夹具：**一份宿主声明 → 这个会话的工具表 → 三家各自的请求体与前缀镜像**。
//!
//! 为什么这套夹具落在 `agent-runtime` 的集成测试里：这条链的两头分别是
//! `ToolTable::with_host_tools`（agent-runtime）和 `Provider::encode`
//! （agent-providers），只有这个 crate 同时看得见两边——而 063 要钉的正是「客户端
//! 给的那份声明，最后变成了哪些字节」。
//!
//! # 两个落点，一条断言链
//!
//! - **wire**：请求体里 `tools` 那个数组的原始字节（[`wire_tools_bytes`]）。
//! - **前缀镜像**：`Encoded::prefix` 里 Tools 那一段的 `bytes`/`hash`
//!   （[`tools_segment`]）——`wire/prefix.rs` 的 `SegmentBytes.tools` 唯一对外可见的
//!   投影，红线 11 那笔钱（判「前缀漂没漂」）算在它身上。
//!
//! 两者是不是同一份字节，由 `host_tools_prefix_is_byte_deterministic.rs` 里那条
//! tie 断言（[`hash`] vs `SegmentImage.hash`）看住。

#![allow(dead_code, unused_imports)]

use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;
use std::sync::{Arc, OnceLock};

use agent_core::{
    PrefixImage, RequestIntent, Reversibility, Segment, SegmentImage, SessionConfig, ToolSpec,
};
use agent_providers::deepseek::DeepSeek;
use agent_providers::glm::Glm;
use agent_providers::kimi::Kimi;
use agent_providers::{Encoded, Ingredients, Provider};
use agent_runtime::ToolTable;
use serde_json::Value;

/// 12 个声明，**故意不按字典序给**（`web:`/`desk:` 交替，同前缀内也乱序）。
///
/// 12 个而不是 2 个是为了突变验证能稳定复现：把 `with_host_tools` 的排序换成
/// `HashMap` 迭代那种改法，2 个元素有一半概率碰巧还是同序，12 个就不会。
///
/// 描述与 schema 里**不许出现 `"tools":` 这个字面串**——[`wire_tools_bytes`] 靠它
/// 在请求体里定位顶层的工具段。
pub const DECLARED: &str = r#"[
  { "name": "web:crm/lookup",       "description": "按客户 ID 查档案", "schema": { "type": "object" } },
  { "name": "desk:clipboard/write", "description": "写系统剪贴板",     "schema": { "type": "object" } },
  { "name": "web:crm/draft",        "description": "起草一封邮件",     "schema": { "type": "object" } },
  { "name": "desk:mail/send",       "description": "发一封邮件",       "schema": { "type": "object" } },
  { "name": "web:crm/close",        "description": "关掉一个工单",     "schema": { "type": "object" } },
  { "name": "desk:fs/reveal",       "description": "在访达里显示",     "schema": { "type": "object" } },
  { "name": "web:report/render",    "description": "渲染一张报表",     "schema": { "type": "object" } },
  { "name": "desk:printer/print",   "description": "打印",             "schema": { "type": "object" } },
  { "name": "web:auth/whoami",      "description": "当前登录的是谁",   "schema": { "type": "object" } },
  { "name": "desk:notify/toast",    "description": "弹一条通知",       "schema": { "type": "object" } },
  { "name": "web:calendar/book",    "description": "占一个会议室",     "schema": { "type": "object" } },
  { "name": "desk:shell/open",      "description": "打开一个外部程序", "schema": { "type": "object" } }
]"#;

/// 镜像那一段字节在 `deepseek`/`glm`/`kimi` 三个 `encode.rs` 里**各写了一遍**
/// （`prefix::SegmentBytes { tools: canonical(&built.value), .. }`），所以每条断言都
/// 对三家各跑一遍：只测一家的话，另外两家哪天改成另算一份都没人拦得住。
pub fn providers() -> Vec<(&'static str, Box<dyn Provider>)> {
    vec![
        ("deepseek", Box::new(DeepSeek)),
        ("glm", Box::new(Glm)),
        ("kimi", Box::new(Kimi)),
    ]
}

fn config() -> &'static SessionConfig {
    static CONFIG: OnceLock<SessionConfig> = OnceLock::new();
    CONFIG.get_or_init(|| SessionConfig {
        model: Arc::from("determinism-fixture"),
        temperature: None,
        max_tokens: None,
        context_window: None,
    })
}

/// 一次组装。料单里除了 `tools` 全是空的——本 issue 只看工具表那一段字节，
/// 掺 system/history 进来只会让失败信息变难读。
pub fn encode(provider: &dyn Provider, tools: &[ToolSpec], prev: Option<&PrefixImage>) -> Encoded {
    provider.encode(&Ingredients {
        system: &[],
        messages: &[],
        tools,
        late_tools: &[],
        late_system: &[],
        config: config(),
        intent: RequestIntent::Free,
        prev_prefix: prev,
    })
}

/// 宿主声明的 JSON **文本** → `(ToolSpec, Reversibility)`：跟 `agent-server` 的
/// `http::capabilities::assemble::host_tools` 同一条翻译（三个字段原样进 `ToolSpec`，
/// `schema` 一个字节不改写）。那个函数是 `pub(in crate::http)`，跨 crate 够不着，所以
/// 这里照它的形状解一遍。
///
/// **故意从文本解**、不用 `json!` 宏直接造 `Value`：客户端交上来的就是文本，
/// schema 的 key 序确定性只有从文本进来才测得到（063 §注意）。可逆性不进 prompt，
/// 这里一律给保守值，本 issue 不看它。
pub fn declaration(text: &str) -> Vec<(ToolSpec, Reversibility)> {
    let parsed: Value = serde_json::from_str(text).expect("夹具里的声明该是合法 JSON");
    parsed
        .as_array()
        .expect("声明是个数组")
        .iter()
        .map(|tool| {
            let spec = ToolSpec {
                name: Arc::from(tool["name"].as_str().expect("每条声明都有 name")),
                description: Arc::from(tool["description"].as_str().unwrap_or_default()),
                schema: Arc::new(tool["schema"].clone()),
            };
            (spec, Reversibility::Irreversible)
        })
        .collect()
}

/// 部署期那一档 + 这一次声明的注入，形状照 `agent-server` 的装配链
/// （`spec.tools.build().with_host_tools(spec.host_tools)`）。
pub fn table_with(decl: &str) -> ToolTable {
    ToolTable::with_shell().with_host_tools(declaration(decl))
}

/// 同一档、**没有任何注入**的表：三条里第 3 条的基线。
pub fn baseline_table() -> ToolTable {
    ToolTable::with_shell()
}

/// 把声明数组倒过来——同一份声明的另一种数组顺序。
pub fn reversed(decl: &str) -> String {
    let mut parsed: Value = serde_json::from_str(decl).expect("合法 JSON");
    parsed.as_array_mut().expect("数组").reverse();
    parsed.to_string()
}

/// 把声明数组左移 `n` 位——再来一种数组顺序（倒序太规整，轮转能盖住「碰巧对称」）。
pub fn rotated(decl: &str, n: usize) -> String {
    let mut parsed: Value = serde_json::from_str(decl).expect("合法 JSON");
    parsed.as_array_mut().expect("数组").rotate_left(n);
    parsed.to_string()
}

/// 请求体里 `tools` 那个数组的**原始字节**——从 `body` 里切出来的，不是解析回来再
/// 序列化一次（那样切法本身就会把顺序问题洗掉）。
///
/// 右边界不靠「`tools` 是不是最后一个 key」这种假设：定位 `"tools":` 之后用
/// `serde_json` 自己的流式反序列化读**一个**值，`byte_offset()` 就是它的右边界。
pub fn wire_tools_bytes(enc: &Encoded) -> &[u8] {
    const KEY: &[u8] = b"\"tools\":";
    let at = enc
        .body
        .windows(KEY.len())
        .position(|w| w == KEY)
        .unwrap_or_else(|| {
            panic!("请求体里没有 tools 段：{}", text(&enc.body));
        });
    let start = at + KEY.len();
    let mut stream = serde_json::Deserializer::from_slice(&enc.body[start..]).into_iter::<Value>();
    let value = stream
        .next()
        .expect("tools 后面该有一个值")
        .expect("该是合法 JSON");
    assert!(
        value.is_array(),
        "定位到的 tools 段不是数组，夹具里大概混进了 `\"tools\":` 字面串"
    );
    &enc.body[start..start + stream.byte_offset()]
}

/// 前缀镜像里 Tools 那一段。
pub fn tools_segment(prefix: &PrefixImage) -> &SegmentImage {
    prefix
        .segments
        .iter()
        .find(|s| s.segment == Segment::Tools)
        .expect("镜像里该有 Tools 段")
}

/// `agent-providers` 里 `wire::prefix::hash` 的**同款复制**（`DefaultHasher`，固定种子）。
///
/// 那个模块是 `pub(crate)`，跨 crate 拿不到；而「镜像和 wire 是不是同一份字节」这条
/// 只有把 wire 字节按同一个算法算一遍、跟 `SegmentImage.hash` 比才证得了——只比长度
/// 的话，「镜像那边把数组倒过来」这种改法长度不变、照样滑过去。
///
/// **哪天 `wire/prefix.rs` 换了哈希算法，这里要跟着换**：那时 tie 那条会红，
/// 失败信息就是这段注释。
pub fn hash(bytes: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    h.write(bytes);
    h.finish()
}

/// 一段 `tools` 数组字节 → 逐项的字节。
///
/// 顺带把拆法本身证一遍：拆出来的项用 `,` 接回去、套上方括号，必须跟原始字节一模
/// 一样——否则「逐项字节相同」推不出「那一段字节相同」。
pub fn items(tools_bytes: &[u8]) -> Vec<Vec<u8>> {
    let parsed: Value = serde_json::from_slice(tools_bytes).expect("该是合法 JSON 数组");
    let items: Vec<Vec<u8>> = parsed
        .as_array()
        .expect("数组")
        .iter()
        .map(|item| serde_json::to_vec(item).expect("Value 序列化不会失败"))
        .collect();

    let mut rebuilt = vec![b'['];
    for (n, item) in items.iter().enumerate() {
        if n > 0 {
            rebuilt.push(b',');
        }
        rebuilt.extend_from_slice(item);
    }
    rebuilt.push(b']');
    assert_eq!(
        text(&rebuilt),
        text(tools_bytes),
        "拆项再拼回去必须逐字节还原"
    );
    items
}

/// 断言失败信息里给人看的形态（字节数组 `Debug` 出来没法读）。
pub fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// 逐字节相等，失败时只打**第一处分歧**前后各 60 字节。
///
/// 不用 `assert_eq!` 两大坨字符串：一张真实工具表有几千字节，突变验证第一次跑出来的
/// 失败信息滚了两屏、看不出到底哪儿变了。定位到「从第几位起不一样」才是能用的信息。
pub fn assert_same_bytes(label: &str, expected: &[u8], actual: &[u8]) {
    if expected == actual {
        return;
    }
    let at = expected
        .iter()
        .zip(actual)
        .position(|(a, b)| a != b)
        .unwrap_or_else(|| expected.len().min(actual.len()));
    let from = at.saturating_sub(60);
    panic!(
        "{label}：字节从第 {at} 位起就不一样了（左 {} 字节 / 右 {} 字节）\n  左：…{}…\n  右：…{}…",
        expected.len(),
        actual.len(),
        text(&expected[from..(at + 60).min(expected.len())]),
        text(&actual[from..(at + 60).min(actual.len())]),
    );
}
