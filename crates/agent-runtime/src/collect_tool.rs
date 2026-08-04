//! `srv:agent/collect`：模型用来**领取**一个后台子 agent 最终结果的那个工具
//! （M8，docs/ORCHESTRATION.md §三）。052 是发射半边，这里是收割半边。
//!
//! # 它没有自己的等待机制——等待就是那个还没收敛的槽
//!
//! 两条出路，都不需要新机械：
//!
//! | 子的状态 | collect 干什么 | 父的槽 |
//! |---|---|---|
//! | 已跑完，结果躺在 stash 里 | 当场端走（**领取即消费**） | 立刻收敛 |
//! | 还在跑 | 往 `Subtree` 的槽位表补一笔，绑到这次 collect 的 `call_id` | 保持 `Pending`，收割时回写 |
//!
//! 第二条**跟前台阻塞 spawn 逐字同一条路**（`Subtree::harvest_slots`）：父停在
//! `ToolsPending`、泵接着把子驱动到终态、子的正文经 `Event::ToolResult` 回到这个
//! 槽。ORCHESTRATION §三 那句「前台 spawn ≡ spawn(bg) + 紧跟 collect」说的就是
//! 这件事——后台把 spawn 和 collect 拆成两次调用，中间塞得进 `status` 和别的活，
//! 决策 20 是这个模型的一个特例，不是被推翻。
//!
//! # 可逆性 `Pure`，且**不需要**自己的屏障位
//!
//! collect 只读一份已经产生的结果，不写任何会话状态、不落命令、没有补偿动作。
//! 「可子 agent 会去干不可逆的事啊」——那些事各自带**它自己的**屏障位：子跑
//! `srv:shell/exec` 时记录那条结果的 entry 就是 `barrier: true`，而它跟父这条
//! collect 在**同一条日志、同一个 turn_id** 上（决策 5）。undo 往回走会先撞上子
//! 那条屏障停下来问，轮不到 collect。跟 spawn 判 `Reversible` 是同一套账
//! （`tool_table::reversibility_of` 的注释记着）。
//!
//! # 红线 10：`id` 必须是调用者的后代
//!
//! 拒绝文本里把「你现在能领的是哪些」一并给出——而那份清单同样只列调用者的后代
//! （[`mine`]），所以**结构上无从放大视野**：既看不到兄弟的后台子，也不会因为
//! 一句好心的提示把别人的 agent id 漏给模型。
//!
//! 子的正文经 `crate::child_outcome` 读、经 `Event::ToolResult` 从正门写回父——
//! 运行时侧读，不经 core 的跨 agent 读 API（`Messages` 是 Upward-only，core 那层
//! 根本读不到），见 ORCHESTRATION §五。

use std::sync::Arc;

use agent_core::{AgentId, Epoch, Session, ToolCallId, ToolSpec};
use serde_json::{Value, json};

use crate::ctx::RunnerCtx;
use crate::dispatch::Dispatched;
use crate::event::RunnerEvent;
use crate::reply;
use crate::subtree::Subtree;

/// 工具全名。`srv:` = 服务端本地执行（`Location::Server`，docs/TOOLS.md 的命名
/// 约定），`agent/collect` = 这一族里的 collect。
pub const COLLECT_TOOL: &str = "srv:agent/collect";

/// 喂给模型的声明。
pub fn collect_spec() -> ToolSpec {
    ToolSpec {
        name: Arc::from(COLLECT_TOOL),
        description: Arc::from(
            "领取一个你用 background=true 开出去的子 agent 的最终结果。\n\
             它已经跑完了 → 立刻返回它的回答；还在跑 → 这次调用等它跑完再返回\
             （和不带 background 的 spawn 一样会等，只是等的时机由你挑）。\n\
             一个子 agent 的结果**只能领一次**：领过的、或者本来就不是你用 \
             background=true 开的，这次调用会返回错误（不影响你继续干别的）。\n\
             **你这一轮结束前没领的后台子 agent 会被拆掉、结果丢弃**——开了后台就\
             记得回来领。\n\
             什么时候用：先用 srv:agent/status 看谁已经 Done，就先 collect 谁，\
             按「谁先完先用谁」推进，而不是按 spawn 的顺序死等第一个。",
        ),
        schema: Arc::new(json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "要领哪个子 agent 的结果，比如 root/a1——就是那次 background=true 的 spawn 返回给你的 agent_id。必须是你的后代。"
                }
            },
            "required": ["id"]
        })),
    }
}

/// 截获一次 `srv:agent/collect`。
///
/// **不调 `persist::sync`**（跟 `status_tool::intercept` 同一条理由）：这里一条
/// 命令都没发，`Subtree` 的三张表是泵的 turn 内局部记账，没有任何 `Entry` 需要
/// 同步进持久化后端。真正落日志的是随后那条经 `Session::step` 的 `ToolResult`，
/// 泵在 A 段自己会转发。
///
/// 失败（入参写错、不是自己的后代、领过了……）→ `is_error` 的 tool_result 喂回
/// 模型让它自己收敛（决策 20 的哲学，跟 spawn/status 一致），不 panic、不卡这一轮。
pub(crate) fn intercept(
    session: &Session,
    ctx: &mut RunnerCtx,
    subtree: &mut Subtree,
    agent: &AgentId,
    call_id: ToolCallId,
    input: &Arc<Value>,
    epoch: Epoch,
) -> Dispatched {
    // collect 也是一次工具调用，该跟别的工具一样看得见「调了什么、参数是什么」。
    let request = ctx.tools.snapshot(COLLECT_TOOL, Arc::clone(input));
    ctx.emit(agent, RunnerEvent::ToolExecuting { call_id: call_id.clone(), request });

    let child = match parse(input) {
        Ok(child) => child,
        Err(message) => return refuse(ctx, agent, call_id, epoch, message),
    };
    if !child.is_descendant_of(agent) {
        // 祖先、兄弟、别的树上的 id，以及**调用者自己**都落在这里（红线 10）。
        let message = not_a_descendant(agent, &child, subtree);
        return refuse(ctx, agent, call_id, epoch, message);
    }

    // 一：已完成未领取 —— 端走它，**同时从 stash 划掉**。第二次 collect 同一个 id
    // 因此落到下面那条「领不了」的路上，拿到 `is_error`：一份结果只能领一次。
    if let Some(done) = subtree.take_stashed(&child) {
        let body = done.content.to_string();
        return reply::settle(ctx, agent, call_id, epoch, COLLECT_TOOL, body, done.is_error);
    }

    // 二：还在跑 —— 绑到这次 collect 的槽上，返回 `Nothing`（不产出任何事件），
    // 父那个槽因此保持 `Pending`、父停在 `ToolsPending`，泵接着把子驱动到终态，
    // `Subtree::harvest_slots` 再把正文回写进来。
    //
    // `is_live` 是防死等的那一道：detached 名单上一个已经被拆掉/撤销的子永远不会
    // 落终态，绑上去就是让父等一个不会来的结果（`run_turn` 里没有人能救它）。
    if subtree.is_detached(&child) && session.is_live(&child) {
        if subtree.is_awaited(&child) {
            let message = already_awaited(&child);
            return refuse(ctx, agent, call_id, epoch, message);
        }
        subtree.record(child, agent.clone(), call_id, epoch, COLLECT_TOOL);
        return Dispatched::Nothing;
    }

    // 三：领不了。
    let message = not_collectable(&child, agent, subtree);
    refuse(ctx, agent, call_id, epoch, message)
}

fn refuse(
    ctx: &mut RunnerCtx,
    agent: &AgentId,
    call_id: ToolCallId,
    epoch: Epoch,
    message: String,
) -> Dispatched {
    reply::refuse(ctx, agent, call_id, epoch, COLLECT_TOOL, message)
}

/// 模型给的入参解析结果：要领哪一个。
///
/// **错误一律是给模型看的文本**（跟 `spawn_tool::parse` / `status_tool::parse`
/// 一致）：入参是模型写的，写错了该让它自己看见并改（003 的哲学）。
///
/// `id` 是**必填**——这一点跟 `status` 的可选 `id` 不同，也不该「省掉就领一个」：
/// 领哪个是有后果的选择（领了就消费掉了），替模型猜一个是最不该省的那种省事。
pub(crate) fn parse(input: &Value) -> Result<AgentId, String> {
    match input.get("id") {
        Some(Value::String(id)) if !id.trim().is_empty() => Ok(AgentId::new(id.trim())),
        Some(Value::String(_)) => Err("collect 失败：id 是空的。".to_string()),
        None | Some(Value::Null) => Err(
            "collect 失败：缺少必填参数 id（要领哪个后台子 agent 的结果，比如 root/a1）。"
                .to_string(),
        ),
        Some(_) => {
            Err("collect 失败：id 得是字符串（agent 的路径 id，比如 root/a1）。".to_string())
        }
    }
}

/// 此刻**这个调用者**领得动的后代，按 `AgentId` 排序（`Subtree::collectable`
/// 排的）。红线 10 由构造保证：清单本身就是「我的后代」这一个集合，拒绝文本
/// 放不大视野。
fn mine<'a>(subtree: &'a Subtree, caller: &AgentId) -> Vec<&'a AgentId> {
    subtree.collectable().into_iter().filter(|id| id.is_descendant_of(caller)).collect()
}

/// 横读/上读被拒。**说清是谁、并把「你能领的是哪些」一并给出**——模型才知道下一步
/// 该领谁（跟 `status_tool::not_a_descendant` 同一个写法）。
fn not_a_descendant(caller: &AgentId, child: &AgentId, subtree: &Subtree) -> String {
    format!(
        "collect 失败：{} 不是你（{}）的后代。你只能领自己开出去的子 agent 的结果，\
         领不到自己、祖先和兄弟的。{}",
        child.as_str(),
        caller.as_str(),
        you_can_collect(&mine(subtree, caller)),
    )
}

/// 同一个子被 collect 两次，而**第一次还没回来**（同一条 assistant 消息里发了两次，
/// 或者上一次绑定还在等）。不重复绑：两个槽等同一个子会让同一份结果回写两遍，
/// 而模型要的答案第一次就会到。
fn already_awaited(child: &AgentId) -> String {
    format!(
        "collect 失败：你已经在领 {} 了，那次调用还没回来——等它就行，别重复领。",
        child.as_str(),
    )
}

/// 形状对（确实是自己的后代）但领不动：领过了、不是后台开的、或者已经被撤销/拆掉。
///
/// **三种情形合成一句**而不是分三条：它们在这一刻是分不开的（`Subtree` 里都表现为
/// 「两张表里都没有」），硬分就得为「已经领过」再留一张只为措辞而存在的表，而那张
/// 表自己也会跟真相不同步。诚实地把三种可能都列出来，比精确地猜错一种好。
fn not_collectable(child: &AgentId, caller: &AgentId, subtree: &Subtree) -> String {
    format!(
        "collect 失败：{} 不在你还没领的后台子 agent 里。可能是你已经领过它了\
         （一份结果只能领一次），可能它不是用 background=true 开的（那种 spawn 的\
         结果在那次调用里就回给你了），也可能它已经被撤销或拆掉了。{}",
        child.as_str(),
        you_can_collect(&mine(subtree, caller)),
    )
}

/// 拒绝文本的后半句。它也进 prompt，所以顺序照样是 `Subtree::collectable` 排好的
/// 那个（红线 11）。
fn you_can_collect(mine: &[&AgentId]) -> String {
    if mine.is_empty() {
        return "你现在没有等着领的后台子 agent。".to_string();
    }
    format!(
        "你现在能领的是：{}。",
        mine.iter().map(|id| id.as_str()).collect::<Vec<_>>().join("、"),
    )
}

#[cfg(test)]
#[path = "collect_tool_tests.rs"]
mod tests;
