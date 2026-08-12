//! `srv:agent/spawn`：模型用来把一件事拆给子 agent 的那个工具（决策 20）。
//!
//! # 为什么它的声明在 `agent-runtime`，不在 `agent-tools`
//!
//! `agent-tools` 的 `builtin_specs()` 里那几个，全部是 `ToolExecutor::execute`
//! 分发得掉的东西——给一个名字和一份 JSON，它跑完返回一段文本。spawn **不是**
//! 那种工具：它要改的是会话状态（长出一个 agent、记一条 entry），而
//! `ToolExecutor` 既够不着 `Session` 也够不着泵。把它塞进 `builtin_specs()` 只会
//! 得到一个「声明在 A、执行在 B、A 那边永远 `unknown_tool`」的分裂形状。
//!
//! 它的执行点是宿主侧的一次**截获**（`crate::dispatch`：`ExecuteTool` 分派处按
//! 工具名 match 到本文件的 [`intercept`]），而宿主本来就持有工具表——按名字分流
//! 在这一层是合法的，跟红线 12 禁的「core 里按 provider 分支」不是一回事：这里
//! 没有模型相关判断，只有「这个名字归谁执行」。
//!
//! # 截获实现住在这里，不住 `dispatch.rs`（053 的前置重构）
//!
//! 051 的 `status_tool::intercept` 立的规矩：`dispatch.rs` 只回答「这个名字归谁」，
//! 「怎么执行」跟着工具本身走。052 之前 spawn 是唯一的例外（它的三个函数住在
//! `dispatch.rs`），053 要在那里加第五处截获时它已经贴着红线 9 的 300 行——所以
//! 把 spawn 这三个函数搬回自己家，`dispatch.rs` 回到纯分派器的形状。
//!
//! # 上限进描述，是给模型看的（029：「描述写给模型看」）
//!
//! 决策 20 的兜底是「超限 = `is_error` 的 tool_result 让模型自己收敛」——但先
//! 告诉它上限是多少，能省掉大部分那种往返。数字来自 [`AgentLimits`]，宿主建
//! 工具表时传进来，跟 `Session` 手上那份是同一组数（`ToolTable::with_spawn` 的
//! 文档记了这个耦合）。
//!
//! # `tools` 子集里的名字两种拼法都认（050）
//!
//! 模型在函数列表里看到的工具名是转义过的（`srv_3Afs_2Flist`），照抄进参数里是它
//! 唯一能做的事。归一化在 [`check_subset`]，规则与理由见 `crate::tool_name`；
//! 描述里那句「照抄你工具列表里的那个名字」因此是句真话，不是一句它做不到的要求。

use std::sync::Arc;

use agent_core::{AgentId, ChildConfig, Epoch, Event, Session, SpawnRefused, ToolCallId};
use serde_json::{Value, json};

use crate::ctx::RunnerCtx;
use crate::dispatch::Dispatched;
use crate::event::RunnerEvent;
pub(crate) use crate::spawn_request::SPAWN_TOOL;
use crate::spawn_request::parse;
use crate::subtree::Subtree;
use crate::{persist, reply, subagent, tool_name};

/// 模型点名的那几个工具 → 父手上那几个**规范全名**，或者一句拒绝。两件事一次做完：
///
/// 1. **归一化**（050，规则与理由见 `crate::tool_name`）。落进 `ChildConfig` 的
///    必须是规范名：子 agent 的工具表是拿它去**精确**过滤宿主那张表的
///    （`subagent::tools_for`），wire 名进去 = 子 agent 一个工具都没有。
/// 2. **提权拦截**。父自己没有的（两种拼法都不是）→ 拒绝，并把「你有哪些」一并
///    告诉它。不静默过滤：静默过滤出来的子 agent 会带着一份跟模型以为的不一样的
///    工具表干活，然后在子 agent 那边报一个跟 spawn 毫无关系的 `unknown_tool`。
pub(crate) fn check_subset(
    wanted: &[Arc<str>],
    parent_has: &[Arc<str>],
) -> Result<Vec<Arc<str>>, String> {
    let mut resolved = Vec::with_capacity(wanted.len());
    let mut missing: Vec<&str> = Vec::new();
    for name in wanted {
        match tool_name::resolve(name, parent_has) {
            Some(hit) => resolved.push(Arc::clone(hit)),
            // 原样回显模型写的那个字符串，不回显解码结果——它要认出自己写错了什么。
            None => missing.push(name),
        }
    }
    if missing.is_empty() {
        return Ok(resolved);
    }
    Err(format!(
        "spawn 失败：你要给子 agent 的这些工具你自己没有：{}。你现在有的是：{}。",
        missing.join("、"),
        parent_has
            .iter()
            .map(|n| &**n)
            .collect::<Vec<_>>()
            .join("、"),
    ))
}

/// [`SpawnRefused`] → 给模型看的一句话。**说清是哪一条闸、当前的数字是多少**，
/// 模型才知道该怎么收敛（决策 20：让它自己收敛）。
pub(crate) fn refusal_text(refused: &SpawnRefused) -> String {
    match refused {
        SpawnRefused::DepthExceeded { depth, max } => format!(
            "spawn 失败：agent 树深度上限是 {max}，这个子 agent 会落在深度 {depth}。\
             这一层不能再往下拆了，剩下的自己做。"
        ),
        SpawnRefused::TooManyChildren { live, max } => format!(
            "spawn 失败：每个 agent 最多 {max} 个活着的直接子 agent，你已经有 {live} 个。\
             等手上这些回来之后再拆，或者少拆几个。"
        ),
        // 下面两条是宿主侧的 bug（父 agent 不在这棵树上 / 已经不活着），不是模型
        // 能收敛的东西——照样如实回给它，让这一轮有个结果而不是卡住。
        SpawnRefused::NotInSession { parent } => {
            format!(
                "spawn 失败：发起 spawn 的 agent（{}）不在这个会话的 agent 树上。",
                parent.as_str()
            )
        }
        SpawnRefused::ParentNotLive { parent } => {
            format!(
                "spawn 失败：发起 spawn 的 agent（{}）已经不在活名单上了。",
                parent.as_str()
            )
        }
    }
}

/// 截获一次 `srv:agent/spawn`。
///
/// **前台**（`background=false`，缺省）：父那个槽**保持 `Pending`**——这条路不
/// 产出任何喂给父的事件，只产出子 agent 的第一条 `UserInput`。父的等待因此不是
/// 一段等待代码，就是那个还没收敛的槽位（006 决策记录）。收敛发生在子 agent 落
/// 终态时，见 `crate::subtree`。
///
/// **后台**（`background=true`，052）：见 [`detach`]。分岔只在最后一步，前面的
/// 解析/子集校验/建子/持久化**逐字节共用**——决策 20 的那条路一行没改。
pub(crate) fn intercept(
    session: &mut Session,
    ctx: &mut RunnerCtx,
    subtree: &mut Subtree,
    parent: &AgentId,
    call_id: ToolCallId,
    input: &Arc<Value>,
    epoch: Epoch,
) -> Dispatched {
    // spawn 也是一次工具调用，该跟别的工具一样看得见「调了什么、参数是什么」。
    let request = ctx.tools.snapshot(SPAWN_TOOL, Arc::clone(input));
    ctx.emit(
        parent,
        RunnerEvent::ToolExecuting {
            call_id: call_id.clone(),
            request,
        },
    );

    let parsed = match parse(input) {
        Ok(parsed) => parsed,
        Err(message) => return reply::refuse(ctx, parent, call_id, epoch, SPAWN_TOOL, message),
    };
    let parent_tools = subagent::allowed_names(session, ctx, parent);
    let tools_allowed = match parsed.tools {
        Some(wanted) => match check_subset(&wanted, &parent_tools) {
            Ok(resolved) => resolved,
            Err(message) => {
                return reply::refuse(ctx, parent, call_id, epoch, SPAWN_TOOL, message);
            }
        },
        None => parent_tools,
    };

    let child = match session.spawn_child(
        parent,
        ChildConfig {
            tools_allowed,
            ..ChildConfig::default()
        },
        None,
    ) {
        Ok(child) => child,
        Err(refused) => {
            let message = refusal_text(&refused);
            return reply::refuse(ctx, parent, call_id, epoch, SPAWN_TOOL, message);
        }
    };
    // `spawn_child` 是一条命令，落了一条 `Entry`——跟 `step` 之后一样立刻转发进
    // 持久化后端，否则进程在子 agent 干活期间崩溃，恢复出来的会话里会有一个
    // 「有工作痕迹但没有出生记录」的 agent。
    persist::sync(ctx, session);
    if parsed.background {
        return detach(ctx, subtree, parent, child, call_id, epoch, parsed.task);
    }
    subtree.record(child.clone(), parent.clone(), call_id, epoch, SPAWN_TOOL);

    // 任务文本 = 子 agent 的第一条 user 消息（029 §注意）。它经 `Session::step`
    // 的正门进去：子 agent 刚建好时槽位全是默认值，`Status` 就是 `Idle`，于是
    // 转移表 `Idle + UserInput` 这一格原样接住它、发出它自己的 `CallProvider`。
    // 「子 agent 怎么开始干活」因此没有专门的代码路径。
    Dispatched::Event(Event::UserInput {
        agent: child,
        text: parsed.task,
    })
}

/// 后台 spawn（052）：**槽当场收敛 + 记进 detached 名单**。
///
/// 两条事件按这个顺序喂回泵：
///
/// 1. 给**父**的 `ToolResult`——正文是一个只装 `agent_id` 的 JSON。父的 spawn 槽
///    这一下就收敛了，`ToolsPending` 随之解开，父在**同一个 turn 里**接着发下一
///    个工具调用/下一跳请求，不被这个子挡住。这就是 ORCHESTRATION §三 那句
///    「前台 spawn ≡ spawn(bg) + 紧跟 collect」里被拆开的那一刀。
/// 2. 给**子**的 `UserInput`——跟前台那条路逐字一样，子 agent 怎么开工没有第二
///    份代码。
///
/// 父在前：它先解开阻塞，它的下一跳请求和子的第一跳请求这才真的同时在飞。
///
/// 正文只有 `agent_id` 一个字段：它会原样躺在父的历史里进以后每一次请求
/// （红线 11 要求逐字节确定），所以不放任何此刻的状态（「running」下一秒就可能
/// 是假话）——要看子在干啥有 `srv:agent/status`，那是一次现读。
fn detach(
    ctx: &mut RunnerCtx,
    subtree: &mut Subtree,
    parent: &AgentId,
    child: AgentId,
    call_id: ToolCallId,
    epoch: Epoch,
    task: Arc<str>,
) -> Dispatched {
    subtree.detach(child.clone(), parent.clone(), epoch);
    let content = json!({ "agent_id": child.as_str() }).to_string();
    ctx.emit(
        parent,
        RunnerEvent::ToolExecuted {
            call_id: call_id.clone(),
            tool: Arc::from(SPAWN_TOOL),
            output_len: content.len(),
            is_error: false,
        },
    );
    Dispatched::Events(vec![
        Event::ToolResult {
            agent: parent.clone(),
            epoch,
            call_id,
            content: Arc::from(content),
        },
        Event::UserInput {
            agent: child,
            text: task,
        },
    ])
}

#[cfg(test)]
#[path = "spawn_tool_tests.rs"]
mod tests;
