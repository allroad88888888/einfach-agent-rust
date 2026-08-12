//! 宿主声明里每个名字的校验：**白名单 + 拒绝，绝不 sanitize**。纯函数、零 IO——
//! [`validate`] 只吃一份已经解析好的 [`Capabilities`]，不碰文件系统、不碰 registry，
//! 所以这一层能被单独测透（issue 061 §范围条款 3）。
//!
//! # 为什么是拒绝，不是改写
//!
//! 同 055 的 chatid（[`crate::http::routes::sessions::create`] 安全点一）：悄悄把
//! `web:a b` 洗成 `web:a_b`，两个本来不同的声明就撞成了同一个工具名——后一个静默
//! 盖掉前一个，模型调到哪一个取决于数组顺序。**静默串工具比拒绝更坏。** 同一条理由
//! 也是「重名不做后来居上」的理由：宿主自己都没想清楚要哪个，server 替它选一个只会
//! 把问题推到运行时。
//!
//! # 规则表
//!
//! | 项 | 规则 |
//! |---|---|
//! | 工具名前缀 | **必须** `web:` 或 `desk:`；`srv:`/`mcp:`/无前缀一律拒 |
//! | 工具名前缀之后 | 非空、只许 `[A-Za-z0-9_/-]`、全名 ≤ 128 字节 |
//! | skill 自带的工具 | **v1 不支持**：`tools` 非空就整份 400（140，决策 27），不看形状 |
//! | skill id | 非空、只许 `[A-Za-z0-9_-]`、≤ 128 字节 |
//! | 重名 | 整份声明里工具名（只可能来自顶层）全局唯一、skill id 唯一，撞了就拒 |
//! | `capabilities.prefix` 的名字/text | 156，三条规则的具体逻辑在 [`super::validate_prefix`]：名字前缀同上一行；跟工具名共用同一个「全局唯一」集合（内部重名 + 跟 `capabilities.tools` 撞名都拒）；`text` 不许空串 |
//!
//! # 140：skill 为什么不能带 `tools`
//!
//! 决策 27（M15）把 skill 的注入口从「激活 → `late_tools` 进表」整个砍掉，换成
//! `srv:skill/read` 按需取正文（139）。旧口子一没，`skill.tools` 字段在结构上就
//! 无处可去——没有任何时机会把它塞进模型看到的工具表。与其等宿主声明了、静默
//! 丢在一边，不如在**它自己在场的这一次请求**里如实说清楚（069 判据「在最早能报
//! 给作者的点上失败」）：工具想给这个 skill 用，走 `capabilities.tools` 顶层声明。
//!
//! 这条判定排在**逐条前缀/形状校验之前**——工具名再合法也没用，`tools` 一旦非空
//! 就是整份声明的问题，不必先花一轮 I/O 去挑剔它的字符集。
//!
//! **前缀为什么只许 `web:`/`desk:`**：位置从前缀推是既有规则（`agent_runtime` 的
//! `location_of`），而注入进来的工具本来就跑在宿主侧——用这两个既有前缀就等于直接
//! 接上「服务端推 `ToolExecuting` → 宿主 `POST /tool_result`」那条已经通了的远程
//! 工具通道，零新代码。`srv:` 是「服务端进程内执行」，宿主的工具标成 `srv:` 会让
//! dispatch 去本进程里找一个根本不存在的实现；`mcp:` 同理（`location_of` 把它判成
//! `Server`）。前端自己连的 MCP 该叫 `web:mcp-<server>/<tool>`
//! （HOST-CAPABILITIES.md §七）。
//!
//! **skill id 为什么不许 `/` 和 `:`**：它每行一个进常驻索引（`<id>: <描述>`），也是
//! 模型调 `srv:skill/activate` 时原样写回来的参数。字符集收到跟 chatid 同一档，索引
//! 那一行就不可能被 id 里的分隔符或换行撑破。

use std::collections::BTreeSet;
use std::fmt;

use super::validate_prefix::{self, PrefixRejection};
use super::{Capabilities, CapabilityTool};

/// 工具名合法前缀的白名单——这里是唯一一处，加第三个前缀改这一行。`pub(super)`：
/// `validate_prefix.rs`（156）校验 `capabilities.prefix` 的名字前缀时复用同一份
/// 白名单，不重新拍一遍。
pub(super) const TOOL_PREFIXES: [&str; 2] = ["web:", "desk:"];
pub(super) const MAX_TOOL_NAME_LEN: usize = 128;
const MAX_SKILL_ID_LEN: usize = 128;
/// 错误文案里回显名字时的上限：说得清「是哪一项」，又不至于让错误响应变成一面
/// 把任意长的请求体原样弹回去的镜子（同 [`crate::http::json`] 的取舍）。
const ECHO_LIMIT: usize = 64;

/// 这个名字是在哪儿声明的——错误文案要能直接指到那一项。
///
/// 140 之前这里还有一个 `Skill(String)` 变体（`capabilities.skills[..].tools`
/// 里）：skill 自带的工具那时跟顶层过同一条前缀/形状校验。决策 27 把这条路整个
/// 砍掉之后，`check_tool` 只会被 `capabilities.tools` 这一处调用，`Skill` 变体
/// 因此不再有构造点——**留着就是一处会一直编译通过、却再也不会被走到的分支**，
/// 跟本仓最忌讳的「看似可达、实则死路」是同一种形状，删掉比留着诚实。
#[derive(Clone, PartialEq, Eq, Debug)]
pub(in crate::http) enum Origin {
    /// `capabilities.tools` 里——如今是唯一来源。
    TopLevel,
}

/// 一份声明为什么被拒。**结构化**：调用方拿到的是「哪一项 + 为什么」，不是一句
/// 「请求体不合法」。`Display` 就是回给客户端的那段文案。
#[derive(Clone, PartialEq, Eq, Debug)]
pub(in crate::http) enum CapabilityRejection {
    ToolPrefix { origin: Origin, name: String },
    ToolNameShape { origin: Origin, name: String },
    DuplicateTool { origin: Origin, name: String },
    SkillIdShape { id: String },
    DuplicateSkill { id: String },
    /// 140：这个 skill 的 `tools` 非空——v1 不支持，决策 27 把注入口砍了之后它
    /// 已经无处可去。
    SkillCarriesTools { id: String },
    /// 156：`capabilities.prefix` 里一项违规——三条具体规则（前缀/重名/空 text）
    /// 见 [`super::validate_prefix::PrefixRejection`]，这里只转发它的 `Display`。
    Prefix(PrefixRejection),
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Origin::TopLevel => write!(f, "capabilities.tools"),
        }
    }
}

impl fmt::Display for CapabilityRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CapabilityRejection::ToolPrefix { origin, name } => write!(
                f,
                "{origin} 里的工具名 \"{name}\" 必须以 \"web:\" 或 \"desk:\" 开头——注入的工具跑在宿主侧，位置从前缀推；\"srv:\"/\"mcp:\" 是服务端执行的前缀，不接受"
            ),
            CapabilityRejection::ToolNameShape { origin, name } => write!(
                f,
                "{origin} 里的工具名 \"{name}\" 前缀之后只能是 ASCII 字母、数字、连字符、下划线和斜杠，且不能为空，全名最多 {MAX_TOOL_NAME_LEN} 字节"
            ),
            CapabilityRejection::DuplicateTool { origin, name } => write!(
                f,
                "工具名 \"{name}\" 被重复声明（这一次在 {origin}）——重名一律拒绝，不做「后来居上」"
            ),
            CapabilityRejection::SkillIdShape { id } => write!(
                f,
                "skill id \"{id}\" 只能包含 ASCII 字母、数字、连字符和下划线，且不能为空，最多 {MAX_SKILL_ID_LEN} 字节"
            ),
            CapabilityRejection::DuplicateSkill { id } => {
                write!(
                    f,
                    "skill id \"{id}\" 被重复声明——重名一律拒绝，不做「后来居上」"
                )
            }
            CapabilityRejection::SkillCarriesTools { id } => write!(
                f,
                "skill \"{id}\" 带了 tools——v1 不支持 skill 携带工具（决策 27 裁剪），工具请经 capabilities.tools 声明"
            ),
            CapabilityRejection::Prefix(rejection) => write!(f, "{rejection}"),
        }
    }
}

/// 整份声明的校验。第一条违规就返回——错误是给人看的，一次说清一项即可。
///
/// 工具名的唯一性是**全局**的（顶层的 + 每个 skill 自带的放在同一个集合里判）：
/// 它们最后进的是同一张工具表，重名在哪儿发生都是同一个问题。
pub(in crate::http) fn validate(capabilities: &Capabilities) -> Result<(), CapabilityRejection> {
    let mut tool_names: BTreeSet<&str> = BTreeSet::new();
    for tool in &capabilities.tools {
        check_tool(tool, &Origin::TopLevel, &mut tool_names)?;
    }
    // 156：`capabilities.prefix`——name 前缀/重名（含跟上面这份 `tool_names` 撞）/
    // 空 text 三条新规，具体逻辑在 `validate_prefix`（那个文件自己的模块文档解释
    // 了为什么没有跟这里的工具校验挤在一个文件里）。
    validate_prefix::check_prefix(capabilities, &tool_names).map_err(CapabilityRejection::Prefix)?;
    let mut skill_ids: BTreeSet<&str> = BTreeSet::new();
    for skill in &capabilities.skills {
        if !is_valid_skill_id(&skill.id) {
            return Err(CapabilityRejection::SkillIdShape {
                id: elide(&skill.id),
            });
        }
        if !skill_ids.insert(skill.id.as_str()) {
            return Err(CapabilityRejection::DuplicateSkill {
                id: elide(&skill.id),
            });
        }
        // 140：v1 不支持 skill 携带工具（决策 27）——撞在这里，赶在任何一条工具
        // 形状/前缀检查之前。工具名再合法也没用，`tools` 一旦非空就是整份声明的
        // 问题，没必要先花一轮校验去挑剔一个注定要被拒的字段。
        if !skill.tools.is_empty() {
            return Err(CapabilityRejection::SkillCarriesTools {
                id: elide(&skill.id),
            });
        }
    }
    Ok(())
}

/// 一个工具名：前缀 → 字符集/长度 → 重名。`seen` 借的是 `tool.name` 本身，
/// 不复制字符串。
fn check_tool<'a>(
    tool: &'a CapabilityTool,
    origin: &Origin,
    seen: &mut BTreeSet<&'a str>,
) -> Result<(), CapabilityRejection> {
    let name = tool.name.as_str();
    let Some(rest) = TOOL_PREFIXES
        .iter()
        .find_map(|prefix| name.strip_prefix(prefix))
    else {
        return Err(CapabilityRejection::ToolPrefix {
            origin: origin.clone(),
            name: elide(name),
        });
    };
    let shape_ok = !rest.is_empty()
        && name.len() <= MAX_TOOL_NAME_LEN
        && rest
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'/'));
    if !shape_ok {
        return Err(CapabilityRejection::ToolNameShape {
            origin: origin.clone(),
            name: elide(name),
        });
    }
    if !seen.insert(name) {
        return Err(CapabilityRejection::DuplicateTool {
            origin: origin.clone(),
            name: elide(name),
        });
    }
    Ok(())
}

/// 白名单：非空、`[A-Za-z0-9_-]`、≤128 字节——跟 055 的 chatid 同一档（点号、
/// 斜杠、冒号、换行、非 ASCII 一起被同一条规则挡掉）。
fn is_valid_skill_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_SKILL_ID_LEN
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// 回显用的截断（按字符边界，不切碎 UTF-8）。076 的
/// [`builtin_switch`](super::builtin_switch) 也用它——「错误文案不做任意长度的
/// 镜子」这条取舍只该有一处实现。
pub(super) fn elide(text: &str) -> String {
    match text.char_indices().nth(ECHO_LIMIT) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_string(),
    }
}

#[cfg(test)]
#[path = "validate_tests.rs"]
mod tests;
