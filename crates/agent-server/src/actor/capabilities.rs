//! 宿主注入的能力在**这一个会话**里怎么落地（062/064/073/076 四条 issue 汇到的那一
//! 件事）。
//!
//! 从 [`body`](super::body) 分出来的一件事，三步一条线，全在这个文件里：
//!
//! ```text
//! ① 声明从哪来   restored ? 从回放出来的会话状态取 : 这次请求带的
//! ② 变成料       → 这个会话的 ToolTable（先减后加）+ SkillRegistry（索引进 system 段）
//! ③ 写进历史     新建会话才写，journaled，自成一轮
//! ```
//!
//! 076 之后这里管的**不只是加法**：同一条线上还挂着一个减法（这个会话不启用哪些
//! 内置工具）。三步一个不多——它跟声明是同一类东西（会话状态、进 store、恢复时回放、
//! 有历史就不接受改写），差别只在方向。
//!
//! 分出来的判据（红线 9）：`body.rs` 说得清的一句话是「actor 线程跑什么」，而
//! 「注入的能力从哪来、怎么变成表、什么时候落进日志」自己就是一整套要写清理由的
//! 决定（三条 issue 的结论都挂在这里）。混在一起，`body.rs` 的那句话就说不完了。
//!
//! # ①「从哪来」：恢复读历史，新建看请求（073 拍板，064 沿用）
//!
//! > 历史对话记录，不用对工具再注入一次。**历史对话就该跟历史一致，原模原样 100% 复刻。**
//!
//! 恢复出来的会话读它自己那份历史（`Slot::HostTools` / `Slot::HostSkills`，建会话
//! 那一刻 journaled 写进去的），宿主**不必也不该**在重连时再声明一遍：历史对话是在
//! **那一份**表下产生的，用今天的新清单重建就自相矛盾；而且工具表和常驻索引都在
//! prompt 最前面，换一份 = 恢复出来的第一轮前缀全断（红线 11）。
//!
//! skill 这一路还多一条**它独有**的理由：`Slot::SkillsActive` 早就在 store 里了。
//! 声明不落盘，恢复出来就是一份指向空 registry 的激活集——状态说某个 skill 激活着、
//! 展开注入却什么都取不到（查不到的 id 静默跳过），而模型的历史里明明写着它读过那段
//! 正文。**悬空引用 + 静默**，正是本仓最怕的形状。
//!
//! # ②「怎么变成料」：先减后加，加的都追加在尾巴上
//!
//! - **减**（076）：部署期五档建出来之后**第一件事**就是把这个会话关掉的那些整条
//!   剔掉（`ToolTable::without_builtins`），排在 `with_skills`/`with_mcp`/
//!   `with_host_tools` **之前**——减的是**部署方给的那批**，宿主自己声明的能力不受
//!   这个开关影响（那是它自己报进来的，不想给就别报）。
//! - **工具**：部署期五档先建（所有会话逐字节相同的那一段），宿主声明的追加在最后
//!   （连 MCP 之后，红线 11 / HOST-CAPABILITIES §六）。
//! - **skill**：registry 非空才接 `.with_skills(..)`（那会加两个工具，是所有会话共有
//!   那一段的字节），常驻索引作为一段 `SystemChunk` 追加在部署期那几段之后。
//!
//! **registry 只从声明现造，不合流磁盘 `./skills/`**——069 §拍板「顺带定死 064
//! 第 3 条」，完整理由见 `agent_runtime::SkillRegistry::from_host_skills`。
//!
//! # ③「什么时候写」：`seed_after_recover` **之后**，而且自成一轮
//!
//! 两条顺序都是踩出来的，不是想出来的，见 [`record`]。

use std::sync::Arc;

use agent_core::{HostSkill, Reversibility, Session, SystemChunk, ToolSpec};
use agent_runtime::{RunnerCtx, SkillRegistry, ToolTable};

use crate::registry::OpenSpec;

/// 这个会话的三样 per-session 料：注入的工具、注入的 skill、关掉的内置工具。
///
/// 三样一起从 [`declaration`] 出来，是因为它们的**来源判据只有一个**（`restored`
/// 与否）——分成三个函数各判一次，早晚会出现「工具从历史来、开关却从这次请求来」
/// 这种半新半旧的表，而那种不一致不报错，只在下一次恢复时以少几个工具的形式浮出来。
type Declared = (
    Vec<(ToolSpec, Reversibility)>,
    Vec<HostSkill>,
    Vec<Arc<str>>,
);

/// 这个会话的两段「进 prompt 的料」：工具表 + system 段。
pub(super) struct Assembled {
    pub(super) tools: ToolTable,
    pub(super) system: Vec<SystemChunk>,
}

/// ①+②：确定声明从哪来，装成这个会话的工具表与 system 段。
///
/// `Err(reason)` = **恢复出来的会话又收到了新声明**——第二道闸（第一道在
/// `http::routes::sessions::create`，那里给的是 400 `session_has_history`）。走到
/// 这里说明调用方绕过了 HTTP 层，**不静默忽略**：忽略会让调用方以为登记上了、其实
/// 没有，症状是「模型死活不用某个工具」，离现场十万八千里。两道闸的意义是让「恢复
/// 不接受改写」成为 actor 的性质，而不是「路由记得检查」。
///
/// skill 那一半跟工具**一视同仁**：HTTP 那道闸看的是「带没带 `capabilities`」这个
/// 整体，这里也不能只挡一半。
pub(super) fn assemble(
    spec: &OpenSpec,
    session: &Session,
    restored: bool,
) -> Result<Assembled, String> {
    if restored && !nothing_declared(spec) {
        return Err(format!(
            "session \"{}\" 是从历史恢复出来的，能力只能从历史来：它当初声明的 {} 个工具 / {} 个 skill、关掉的 {} 个内置工具已经在日志里，这次又带了 {} 个工具 / {} 个 skill / {} 个关闭项——恢复是忠实重放，不接受改写（docs/HOST-CAPABILITIES.md §三）",
            spec.id,
            session.host_tools().len(),
            session.host_skills().len(),
            session.disabled_builtins().len(),
            spec.host_tools.len(),
            spec.host_skills.len(),
            spec.disable_builtin.len(),
        ));
    }
    let (host_tools, host_skills, disable_builtin) = declaration(spec, session, restored);
    let skills = SkillRegistry::from_host_skills(host_skills);

    // 常驻索引是 **system 段**的一部分（不是工具）：跟工具表一样随时都在，模型第一
    // 轮、激活之前就能发现有哪些 skill。空 registry → 空文本，
    // `agent_providers` 的 `messages::system_text` 把空段滤掉，于是没声明 skill 的
    // 会话的 system 前缀跟 064 之前**逐字节一致**。追加在部署期那几段之后（红线 11：
    // 既有顺序是契约，只加不改）。
    let mut system = spec.system.clone();
    system.push(skills.skill_index_chunk());

    // **空 registry 时不接 `.with_skills(..)`**：接了就等于给一个没有任何 skill 的
    // 会话平白加两个永远没用的工具（`srv:skill/activate` / `deactivate`），而那一段
    // 字节是所有会话共有的。这也正是 064 §验收「作用域」那条要的——另起一个不带声明
    // 的会话，`srv:skill/activate` **不在表里**。
    //
    // 链式顺序即表的顺序：五档 → **减掉这个会话关掉的那些**（076）→ skill 两件 →
    // 宿主注入（红线 11，§六 第 1 条；跟 `agent-cli` 的 `with_skills` 在
    // `with_host_tools` 之前是同一个次序）。
    //
    // **减排在最前面**：天花板是部署方装配出来的那张表，会话只能在它下面挑。放在
    // 后面就会变成「连宿主自己注入的、连 skill 那两件也能关」——那是两个开关管同一
    // 件事（宿主不想给某个能力，不报它就是了），而两个开关的组合永远会长出说不清的
    // 那一格。`without_builtins` 空列表是一次真正的空操作，不带这个字段的会话工具表
    // 跟 076 之前逐字节相同。
    let tools = spec.tools.build().without_builtins(&disable_builtin);
    let tools = if skills.is_empty() {
        tools
    } else {
        tools.with_skills(skills)
    };
    Ok(Assembled {
        tools: tools.with_host_tools(host_tools),
        system,
    })
}

/// ①：这个会话的声明与开关从哪来。**新建看这次请求，恢复看回放出来的状态。**
fn declaration(spec: &OpenSpec, session: &Session, restored: bool) -> Declared {
    if restored {
        (
            session.host_tools(),
            session.host_skills(),
            session.disabled_builtins(),
        )
    } else {
        (
            spec.host_tools.clone(),
            spec.host_skills.clone(),
            spec.disable_builtin.clone(),
        )
    }
}

/// 这一次请求**一个字都没带**（三样全空）。两处判据共用一个函数：拒绝那一道闸和
/// 「要不要 journaled 地写一次」用的必须是同一句话——分开写的那一刻，就可能出现
/// 「拒绝时算带了、写入时算没带」这种自相矛盾。
fn nothing_declared(spec: &OpenSpec) -> bool {
    spec.host_tools.is_empty() && spec.host_skills.is_empty() && spec.disable_builtin.is_empty()
}

/// ③：全新会话把这一次的声明**journaled 地写一次**，落进这个会话自己的日志，恢复时
/// 跟别的 primitive 一样自动回来。
///
/// **必须排在 `persist::seed_after_recover` 之后**：那一步的语义是「`session` 里现有
/// 的条目本来就在盘上」，声明这两条是本轮新写的，排在它前面就会被当成「已同步」，
/// 从此永远不落盘——恢复时整份声明静默消失，正是 073 要堵的那种洞。
///
/// **声明自成一轮。** 往下的对话从新一轮开始，于是 `/undo` 撤掉第一轮对话不会顺手把
/// 宿主的声明也撤掉——声明是「会话建立」那一步，不是第一轮对话的一部分
/// （`TurnStatus::Idle` 不是终态，`handle_input` 对第一轮不会自己调 `begin_turn`，
/// 不推这一下的话两者会共用 turn 1，一次 `/undo` 连声明一起没了，而且要等到下次重开
/// 工具表才少几个——静默、且离现场很远）。对一个刚建好的会话，`begin_turn` 自己
/// **一个 `Change` 都不产生**，`History::append` 拒绝空步，所以它不落 entry，作用只有
/// 一个：把 turn 边界推过去。
///
/// **没有声明就整段不做**：不带 `capabilities` 的会话因此一条 entry 都不多、turn 号
/// 也不动，会话文件跟 062/064/073/076 之前逐字节相同。三个槽位各自独立判空——只声明了
/// skill 的会话不该平白多一条空的工具声明（`declare_host_*` / `disable_builtins` 传空
/// `Vec` 本来就是空操作，这里的判空是为了连 `begin_turn` 那一下也不做）。
///
/// 076 的开关跟前两样**同进同退**：它也是会话状态（这段历史是在那一份减过的表下产生
/// 的），也 journaled、也自成一轮、也在恢复时从日志回放。
pub(super) fn record(ctx: &mut RunnerCtx, session: &mut Session, spec: &OpenSpec, restored: bool) {
    if restored || nothing_declared(spec) {
        return;
    }
    if !spec.host_tools.is_empty() {
        session.declare_host_tools(spec.host_tools.clone());
    }
    if !spec.host_skills.is_empty() {
        session.declare_host_skills(spec.host_skills.clone());
    }
    if !spec.disable_builtin.is_empty() {
        session.disable_builtins(spec.disable_builtin.clone());
    }
    session.begin_turn();
    agent_runtime::persist::sync(ctx, session);
}
