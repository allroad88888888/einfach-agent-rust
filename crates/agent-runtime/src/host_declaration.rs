//! 一份**宿主工具声明 JSON** → 工具表要的料（122，接缝见
//! `docs/HOST-CAPABILITIES.md` §四/§五）。
//!
//! 从 `tool_table_host.rs` 往前挪一步的那一件事：[`ToolTable::with_host_tools`](super::ToolTable::with_host_tools)
//! 吃的是已经翻好的 `Vec<(ToolSpec, Reversibility)>`，**这里负责把一段外部
//! 文本变成那个 `Vec`**——形状怎么解、什么样的声明当场拒掉、缺省怎么落。装进哪张
//! 表、排在哪一段、可逆性挂在哪儿仍然是 [`ToolTable::with_host_tools`](super::ToolTable::with_host_tools) 的事，这里
//! 不排序、不装配、不认识任何具体宿主。
//!
//! # 形状对齐 `agent-server` 的 `capabilities.tools`，不另起一套
//!
//! 四个字段逐字相同：`name` / `description` / `schema` / `reversibility`
//! （小写三档）。缺字段一律有默认值，认不得的字段忽略不报错——宿主比运行时先升级
//! 是常态。**唯一的差别是范围**：这里只有 `tools`，没有 `skills`、没有
//! `disable_builtin`。那两样在 server 那边各自还牵着一整条路（skill 注册表、
//! 部署期装配出来的那张表），浏览器宿主一条都没有；照抄进来只会得到两个永远为空、
//! 却让页面以为写了就生效的字段。
//!
//! ⚠️ **这是第二份实现，不是共用的一份**：`agent-server` 那份
//! （`http/capabilities/`）绑着 HTTP 请求体、`ts-rs` 导出和另外两个字段，
//! 拆不出来给一个 wasm 宿主用。漂移风险照实说在这里：**四个字段的名字与含义、
//! 三档小写拼法、「没说 = `Irreversible`」这条解释，改一处必须两处一起改。**
//! 校验规则（前缀白名单 `web:`/`desk:`、前缀之后的字符集、128 字节上限、重名一律
//! 拒）同理，两边逐条对着 `http/capabilities/validate.rs` 的规则表写。
//!
//! # 拒绝，不 sanitize；也不整份「跳过坏的那条」
//!
//! 同 `validate.rs`：把 `web:a b` 悄悄洗成 `web:a_b`，两个本来不同的声明就撞成同
//! 一个名字，静默串工具比拒绝更坏。这里再多一条**整份拒**而不是「丢掉不合法的那
//! 几条、剩下的照用」：声明方（页面）拿到的表会跟它以为自己声明的那份不一样，而
//! 工具表就是 prompt 最前面那段字节——它不该在声明方不知情的情况下少几条。
//!
//! # ⚠️ `description` 一个字节都不许动
//!
//! 它**进 prompt**，是模型看得见的文本。这一层不 trim、不改大小写、不补标点、
//! 不做任何规范化：那等于替声明方改了进 prompt 的字节，而且不报错。`schema` 同理
//! 原样收下（`serde_json::Value` 的对象后端是 `BTreeMap`，key 序由它自己定死）。
//!
//! # 红线 11 的责任落在声明方
//!
//! 这一层能保证的只有两件：同一份输入永远转出同一份输出（纯函数），以及字段序/
//! 数组序不会漏进 prompt（`ToolSpec` 的字段顺序由 Rust 类型定死，数组序由
//! [`ToolTable::with_host_tools`](super::ToolTable::with_host_tools) 排掉）。**它保证不了「声明方每次给的是同一份
//! 输入」**——描述文案改一个字、schema 多一个键，前缀缓存当场全断。这条契约必须
//! 由声明入口那一侧写给它的使用者看（浏览器宿主写在 `AgentHost` 构造函数的文档
//! 注释里，那份注释会进生成的 `.d.ts`）。

use std::fmt;
use std::sync::Arc;

use agent_core::{Reversibility, ToolSpec};
use serde::Deserialize;

/// 合法前缀白名单——跟 `agent-server` 的 `validate.rs` 同一行，加第三个前缀两处
/// 一起改。
const TOOL_PREFIXES: [&str; 2] = ["web:", "desk:"];
const MAX_TOOL_NAME_LEN: usize = 128;
/// 错误文案回显名字时的上限：说得清是哪一项，又不至于让错误变成一面把任意长的
/// 输入原样弹回去的镜子（同 `validate.rs` 的 `ECHO_LIMIT`）。
const ECHO_LIMIT: usize = 64;

/// 一份声明为什么被拒。**结构化**：拿到的是「哪一项 + 为什么」，不是一句
/// 「声明不合法」。`Display` 就是可以直接交给声明方的那段文案。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum HostDeclarationError {
    /// 整段 JSON 就没解出来（形状不对、不是合法 JSON）。
    Malformed,
    ToolPrefix {
        name: String,
    },
    ToolNameShape {
        name: String,
    },
    DuplicateTool {
        name: String,
    },
}

impl fmt::Display for HostDeclarationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HostDeclarationError::Malformed => write!(
                f,
                "工具声明 JSON 解析失败：需要 {{\"tools\":[{{\"name\":\"web:…\",\"description\":\"…\",\"schema\":{{…}},\"reversibility\":\"pure|reversible|irreversible\"}}]}}"
            ),
            HostDeclarationError::ToolPrefix { name } => write!(
                f,
                "工具名 \"{name}\" 必须以 \"web:\" 或 \"desk:\" 开头——声明进来的工具跑在宿主侧，位置从前缀推；\"srv:\"/\"mcp:\" 是服务端执行的前缀，不接受"
            ),
            HostDeclarationError::ToolNameShape { name } => write!(
                f,
                "工具名 \"{name}\" 前缀之后只能是 ASCII 字母、数字、连字符、下划线和斜杠，且不能为空，全名最多 {MAX_TOOL_NAME_LEN} 字节"
            ),
            HostDeclarationError::DuplicateTool { name } => write!(
                f,
                "工具名 \"{name}\" 在这份声明里出现了两次——重名一律拒绝，不做「后来居上」"
            ),
        }
    }
}

/// 一份声明 JSON → [`ToolTable::with_host_tools`](super::ToolTable::with_host_tools) 要的料。**纯函数**：同一份输入
/// 永远转出同一份输出，不看时钟、不碰 IO、不做任何规范化。
///
/// 空声明（`{}` / `{"tools":[]}`）→ 空 `Vec`，下游一路空操作。
///
/// 可逆性缺省落保守的 `Irreversible`（HOST-CAPABILITIES §五：「没说」不能推定为
/// 「安全」）；`schema` 缺省 `{"type":"object"}`，跟 server 那侧同一个默认值。
pub fn host_tools_from_declaration(
    json: &str,
) -> Result<Vec<(ToolSpec, Reversibility)>, HostDeclarationError> {
    let declaration = parse_shape(json)?;
    let mut seen: Vec<&str> = Vec::with_capacity(declaration.tools.len());
    for tool in &declaration.tools {
        check_name(&tool.name, &mut seen)?;
    }
    Ok(declaration
        .tools
        .iter()
        .map(|tool| (tool_spec(tool), reversibility(tool)))
        .collect())
}

/// 顶层必须是一个 **JSON 对象**，先过一遍 `Value` 就是为了钉死这一条。
///
/// 直接 `from_str::<HostToolDeclaration>` 会把一个裸数组 `[…]` 也收下——serde 允许
/// 用序列填结构体的字段，而这个结构体的唯一字段有默认值，于是 `[…]` 静默解析成
/// 「一条工具都没声明」。声明方少写一层 `{"tools": …}` 外壳是很容易犯的错，而它的
/// 症状是**一张空工具表加零条错误**：模型突然什么工具都没有，页面这边一声不吭。
fn parse_shape(json: &str) -> Result<HostToolDeclaration, HostDeclarationError> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|_| HostDeclarationError::Malformed)?;
    if !value.is_object() {
        return Err(HostDeclarationError::Malformed);
    }
    serde_json::from_value(value).map_err(|_| HostDeclarationError::Malformed)
}

/// 一个名字：前缀 → 字符集/长度 → 重名。规则表见模块文档，逐条对着
/// `agent-server` 的 `validate.rs`。
fn check_name<'a>(name: &'a str, seen: &mut Vec<&'a str>) -> Result<(), HostDeclarationError> {
    let Some(rest) = TOOL_PREFIXES
        .iter()
        .find_map(|prefix| name.strip_prefix(prefix))
    else {
        return Err(HostDeclarationError::ToolPrefix { name: elide(name) });
    };
    let shape_ok = !rest.is_empty()
        && name.len() <= MAX_TOOL_NAME_LEN
        && rest
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'/'));
    if !shape_ok {
        return Err(HostDeclarationError::ToolNameShape { name: elide(name) });
    }
    if seen.contains(&name) {
        return Err(HostDeclarationError::DuplicateTool { name: elide(name) });
    }
    seen.push(name);
    Ok(())
}

/// 三个**进 prompt** 的字段原样搬——一个字节都不动，理由见模块文档。
fn tool_spec(tool: &DeclaredTool) -> ToolSpec {
    ToolSpec {
        name: Arc::from(tool.name.as_str()),
        description: Arc::from(tool.description.as_str()),
        schema: Arc::new(tool.schema.clone()),
    }
}

/// 声明方省略可逆性时落保守值（HOST-CAPABILITIES §五）。
fn reversibility(tool: &DeclaredTool) -> Reversibility {
    tool.reversibility
        .map_or(Reversibility::Irreversible, Reversibility::from)
}

/// 回显用的截断（按字符边界，不切碎 UTF-8）。
fn elide(text: &str) -> String {
    match text.char_indices().nth(ECHO_LIMIT) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_string(),
    }
}

/// 声明的顶层形状。只有 `tools`——没有 `skills`、没有 `disable_builtin`，理由见
/// 模块文档。认不得的字段忽略（宿主比运行时先升级是常态）。
#[derive(Debug, Default, Deserialize)]
struct HostToolDeclaration {
    #[serde(default)]
    tools: Vec<DeclaredTool>,
}

/// 一条工具声明。四个字段逐字对齐 `agent-server` 的 `CapabilityTool`。
#[derive(Debug, Deserialize)]
struct DeclaredTool {
    /// **必须**带 `web:` / `desk:` 前缀。缺省空串——名字缺了当然要拒，但那一拒该由
    /// [`check_name`] 用一条说得清「哪一项、为什么」的错误来拒，让 serde 去拒只会
    /// 得到一句通用的「形状不符」。
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default = "empty_object_schema")]
    schema: serde_json::Value,
    /// 缺省 `None`：**形状层不替声明方做解释**，落成保守值是
    /// [`reversibility`] 那一步的事。
    #[serde(default)]
    reversibility: Option<DeclaredReversibility>,
}

/// 宿主面的可逆性拼法：**小写**，跟 `agent-server` 的 `CapabilityReversibility`
/// 逐字相同（`agent_core::Reversibility` 的 serde 拼法是 PascalCase，那份已经落进
/// 会话 jsonl 与 TS 类型，宿主面不共用它）。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum DeclaredReversibility {
    Pure,
    Reversible,
    Irreversible,
}

impl From<DeclaredReversibility> for Reversibility {
    fn from(value: DeclaredReversibility) -> Self {
        match value {
            DeclaredReversibility::Pure => Reversibility::Pure,
            DeclaredReversibility::Reversible => Reversibility::Reversible,
            DeclaredReversibility::Irreversible => Reversibility::Irreversible,
        }
    }
}

fn empty_object_schema() -> serde_json::Value {
    serde_json::json!({ "type": "object" })
}

/// 测试用的一步到位：声明 JSON → 这份声明装出来的表 → `specs()` 的序列化字节。
/// **验收「同一份 JSON 转 1000 次逐字节相同」量的就是这串字节**，所以它跟被测的
/// 那条路住在一起，不让每个测试各拼一遍（拼歪一个就白测了）。
#[cfg(test)]
fn declared_table_bytes(json: &str) -> Result<String, HostDeclarationError> {
    let tools = host_tools_from_declaration(json)?;
    let table = super::ToolTable::empty().with_host_tools(tools);
    Ok(serde_json::to_string(table.specs()).expect("ToolSpec 一定序列化得出来"))
}

#[cfg(test)]
#[path = "host_declaration_tests.rs"]
mod tests;
