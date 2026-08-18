//! 浏览器形态的工具表**怎么装出来**：三条 Rust 自己实现的内建，加上页面声明的
//! 那一段（122）。
//!
//! 「一段声明 JSON 怎么解析、什么样的声明当场拒掉」**不在这个文件里**，在
//! `agent_runtime::host_tools_from_declaration`——那是纯逻辑、native 可测（119 §八
//! 「native 可测优先」），这个 crate 是 wasm32 独立 workspace，`cargo test
//! --workspace` 覆盖不到。这里只回答「这份料能不能装进**这张**表」。
//!
//! # 为什么是 `ToolTable::empty()` 而不是 `standard_local()` 减几件
//!
//! 111 决策表第二行：`agent-tools` 的 `srv:` shell/fs specs 在浏览器构建里
//! **不声明**——那些 spec 是纯数据（`pub fn shell_spec()` 只是转发），不声明
//! 模型就压根不知道有它。`ToolTable::empty()` 的文档写着同一条：「公开改写等
//! 不可信输出路径必须从空表开始，不能先装部署期工具再靠名称黑名单回减」。
//! 黑名单回减错一个名字就是一件本不该出现的工具漏进了 prompt，而且**不报错**。
//!
//! 于是业务 `srv:` 工具不出现在 prompt 里是**结构性成立**的，不靠过滤：这张表
//! 从来没有过它们。声明 skill 是明确的唯一例外：`SkillRegistry` 只加入 runtime
//! 实现的 `srv:skill/read`，由模型读取已持久化的 skill 正文；页面声明一条 `srv:`
//! 工具仍会被当场拒掉（建宿主就失败），不是默默接受也不是默默丢掉。
//!
//! # 三条内建为什么保留为「总是存在」，而 121 的脚手架退场
//!
//! 分界线是**谁实现它**，不是谁先写的：
//!
//! | | 实现在 | 声明在 |
//! |---|---|---|
//! | `web:page/title`、`web:page/url`、`web:source/echo` | Rust（[`crate::host_tool::execute`]） | 这里，**不可关闭** |
//! | 别的一切 `web:`/`desk:` 工具 | 页面的工具回调（121） | 页面 |
//!
//! 让页面去声明一条 **Rust 执行**的工具，等于把描述和实现拆到两个地方住：页面把
//! 描述写歪了没有任何人会报错，模型照着错描述用工具，症状出现在离这里很远的地方。
//! 这正是 121 定「内建优先于回调」时的同一条理由，只是这次落在声明侧。
//!
//! 反过来 121 那条 `web:host/callback-probe` 就该退场：它的全部意义是「Rust 侧
//! **不**实现它」，那么它的声明本来就属于页面。122 之后它由 `www/index.html` 的
//! 那份常量声明，121 的四条真机验收原样仍然成立——同一条工具从「Rust 硬编码」
//! 变成「页面声明」，是本条最好的自证。
//!
//! # 派发顺序不会被声明弄坏（两道闸）
//!
//! 1. [`crate::host_tool::execute`] 的 `match` 里三条内建名字排在最前，页面声明
//!    只能进表，进不了那三个分支；
//! 2. 页面声明的名字跟内建**撞名就拒**（[`declare`]）——不是「后来居上」，也不是
//!    「先来的赢、后来的静默丢掉」。静默丢掉的话页面以为自己改写了那条工具的描述，
//!    实际执行的还是 Rust 内建，又是一次「描述与实现对不上且不报错」。
//!
//! # 红线 11：这一段的责任在**页面**
//!
//! 工具表进 prompt 最前面，序列化必须逐字节确定。内建那一段由这个文件保证：
//! 内容是编译期固定的常量，`ToolTable::with_host_tools` 自己会按名字排序再进表
//! （062）。**页面声明那一段保证不了**——`with_host_tools` 只帮忙按名字排序，
//! 帮不了字段顺序和描述文案。
//!
//! > 页面每次刷新交进来的声明 JSON 必须**逐字节一样**：描述文案改一个字、schema
//! > 多一个键、少一条工具，前缀缓存当场全断（DeepSeek 上是 120 倍的差价）。
//! > 正确的写法是把它写成一个**模块级常量**，不是每次现拼的字面量——
//! > `www/index.html` 里的 `PAGE_TOOL_DECLARATION` 就是这么写的。
//!
//! 同一句话也写在 `AgentHost::new` 的文档注释里（那份会进生成的
//! `agent_wasm.d.ts`，页面作者在编辑器里就看得见）。
//!
//! 两段的先后是**内建在前、声明在后**（分两次 `with_host_tools`）：页面声明了
//! 什么都不会挪动内建那三条的字节，两个页面各声明各的，前面那一段仍然逐字节相同。
//!
//! [`tool_table_json`] 把整张表原样吐出来，好让页面（和验收的人）能直接做字节
//! 比对，不必去猜。

use agent_core::{HostSkill, Reversibility, ToolSpec};
use agent_runtime::{SkillRegistry, ToolTable};
use serde_json::json;
use std::sync::Arc;

/// 读 `document.title`。
pub(crate) const PAGE_TITLE_TOOL: &str = "web:page/title";
/// 读 `location.href`。
pub(crate) const PAGE_URL_TOOL: &str = "web:page/url";
/// 124 的验收脚手架，**不是产品能力**：`web:source/` 前缀让它天然走 transient-source
/// 那整套机制（119 §三），回调原样返回入参，用来在真机验收时钉住两件事——真入参/
/// 真结果确实流到了模型，同时历史里只留占位符。落地后可删可留（issue 原话），
/// 留着是因为 130（浏览器识图端到端）要接的正是同一条 `web:source/` 缝，这条先把
/// 缝踩实。执行见 [`crate::host_tool::execute`]。
pub(crate) const SOURCE_ECHO_TOOL: &str = "web:source/echo";

/// Rust 自己实现的那三条。[`declare`] 拿它挡撞名，见模块文档「两道闸」。
const BUILTIN_NAMES: [&str; 3] = [PAGE_TITLE_TOOL, PAGE_URL_TOOL, SOURCE_ECHO_TOOL];

/// 页面交进来的一份声明 JSON → 能装进这张表的料。**页面没给（`None`）或者给了空
/// 声明 → 空 `Vec`**，这个宿主退回到「只有三条内建」，跟 122 之前逐字节相同。
///
/// 两步，各管各的：
///
/// 1. `agent_runtime::host_tools_from_declaration`——这份声明**本身**合法吗
///    （前缀白名单、字符集、长度、声明内部重名、可逆性缺省落 `Irreversible`）；
/// 2. 这里——它能装进**这张**表吗（跟三条内建撞名就拒）。
///
/// 两步都是**整份拒**，不 sanitize、不跳过坏的那条：页面拿到的表要么就是它声明的
/// 那份，要么建宿主当场失败并说清是哪一项。
pub(crate) fn declare(json: Option<&str>) -> Result<Vec<(ToolSpec, Reversibility)>, String> {
    let Some(json) = json else {
        return Ok(Vec::new());
    };
    let declared =
        agent_runtime::host_tools_from_declaration(json).map_err(|error| error.to_string())?;
    for (spec, _) in &declared {
        if BUILTIN_NAMES.contains(&&*spec.name) {
            return Err(format!(
                "工具名 \"{}\" 是这个宿主的内建工具（由 Rust 执行），不能由页面声明——页面声明它只会让描述和实现分居两处，而且不报错",
                spec.name
            ));
        }
    }
    Ok(declared)
}

/// 这个宿主给模型的全部工具：**内建那一段在前，页面声明那一段在后**。见模块文档
/// ——除非声明了 skill，否则没有 `srv:`，绝不装 `mcp:`。
///
/// 分两次 `with_host_tools` 而不是拼成一个 `Vec` 排一次序：内建那一段的字节因此
/// 不随页面声明了什么而挪动（红线 11）。入参已经过 [`declare`]，撞名在那里就拒掉
/// 了，所以这里不会踩到 `push_spec` 的丢弃分支。
///
/// `prefix`（决策 31，157）挂在**链尾**——155 的表尾约定：内置 timed（skills
/// 索引）先注册，声明块的前缀块因此永远排在内置块之后。合成条目不进
/// `specs()`/`declares()`，所以 [`tool_table_json`] 的字节不随它变。
pub(crate) fn browser_tool_table(
    declared: &[(ToolSpec, Reversibility)],
    skills: Vec<HostSkill>,
    prefix: &[(Arc<str>, Arc<str>)],
) -> ToolTable {
    ToolTable::empty()
        .with_host_tools(builtin_tools())
        .with_skills(SkillRegistry::from_host_skills(skills))
        .with_host_tools(declared.to_vec())
        .with_host_prefix(prefix)
}

/// 三条内建 + 各自的可逆性。全是纯读/纯回显，所以 `Pure`。
///
/// **202 起这个 `Pure` 只进显示，不再让 `/undo` 白白越过它们**：它们是 `web:`
/// 工具，执行体在页面里，还原函数交不回来（决策 199 §七）——`/undo` 撞上会停下来
/// 问，用户 `/undo!` 一次越过一条。这三条本来就没有什么要还原的，多问一次是这次
/// 收窄的代价；换来的是没有任何一个宿主工具能靠一句声明绕过那道问询。
///
/// **这不是对页面声明的工具的表态**：那些没说可逆性的一律落保守的
/// `Irreversible`（HOST-CAPABILITIES §五），解释在
/// `agent_runtime::host_tools_from_declaration`。
///
/// 书写顺序无所谓——`with_host_tools` 自己会按名字排序再进表（红线 11）。
fn builtin_tools() -> Vec<(ToolSpec, Reversibility)> {
    vec![
        (
            ToolSpec {
                name: Arc::from(PAGE_TITLE_TOOL),
                description: Arc::from(
                    "读取当前浏览器页面的标题（document.title）。这个信息只有跑在页面里的宿主拿得到，服务端算不出来。无参数。",
                ),
                schema: Arc::new(empty_object_schema()),
            },
            Reversibility::Pure,
        ),
        (
            ToolSpec {
                name: Arc::from(PAGE_URL_TOOL),
                description: Arc::from(
                    "读取当前浏览器页面的地址（location.href）。这个信息只有跑在页面里的宿主拿得到。无参数。",
                ),
                schema: Arc::new(empty_object_schema()),
            },
            Reversibility::Pure,
        ),
        (
            ToolSpec {
                name: Arc::from(SOURCE_ECHO_TOOL),
                description: Arc::from(
                    "验收脚手架：原样返回调用时给的入参。不是给模型日常使用的能力。",
                ),
                schema: Arc::new(echo_schema()),
            },
            Reversibility::Pure,
        ),
    ]
}

/// 无参数工具的 schema。写成一个函数而不是两处各抄一遍，是因为它会进 prompt
/// ——两份字面量哪天被改歪一个，前缀就在没人察觉的情况下漂了（红线 11）。
fn empty_object_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false
    })
}

/// [`SOURCE_ECHO_TOOL`] 的 schema：任意 JSON 对象都收，因为它只是原样吐回去。
fn echo_schema() -> serde_json::Value {
    json!({
        "type": "object"
    })
}

/// 把工具表原样序列化成 JSON：页面可以在关闭前和重开后各取一次做字节比对，且可
/// 检查声明 skill 时只出现 runtime 的 `srv:skill/read`，没有 MCP 或其他业务 `srv:`。
///
/// 这不是「进 prompt 的那份字节」本身（那由各家 adapter 的 `encode` 决定），
/// 但它是 `Ingredients::tools` 的输入，两次相同则那份字节必然相同。
pub(crate) fn tool_table_json(table: &ToolTable) -> String {
    serde_json::to_string_pretty(table.specs())
        .unwrap_or_else(|_| "[\"工具表序列化失败\"]".to_string())
}
