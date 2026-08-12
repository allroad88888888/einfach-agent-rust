//! `Effect::Compact` 的执行点：把「该去把 `[0, upto)` 这段历史摘要了」变成一个
//! 真的窄范围子 agent（106，105 把这里留成了「立刻回 `CompactFailed`」的桩）。
//!
//! 接线点契约（[106](../../../../docs/issues/106-summary-via-subagent.md) 「四条
//! 硬契约」）在这个文件里怎么兑现：
//!
//! 1. **摘要提示词写在这里**（宿主侧），不下沉进 `agent-providers`——
//!    [`SUMMARY_INSTRUCTIONS`] 是产品判断，不是模型判断。
//! 2. **子 agent 用哪个模型来自它自己的 `ChildConfig`**：
//!    `ctx.compaction_execution_profile`（[`crate::ctx::RunnerCtx`] 的一个可选
//!    字段，宿主装配时给）落进 [`ChildConfig::execution_profile`]，`agent-core`
//!    没有为摘要新增任何 provider 分支（红线 12）——这条路复用 093 已经有的
//!    「不透明 profile id → runtime 解析绑定」机制，不新造一套。
//! 3. **spawn 被拒也是 `CompactFailed`**：深度/子数撞顶时
//!    `session.spawn_child` 返回 `Err`，这不是这次压缩独有的 bug，只是运气不好
//!    撞见了资源上限——当场回 `Event::CompactFailed`，不产出子 agent、不占用
//!    [`crate::compact_slot::CompactSlots`] 的任何一格。子 agent 真的跑起来之后
//!    再失败（`CompactSlots::harvest`）在父这一侧看到的是同一种事件，是同一条
//!    「正常事件」路径的两个入口。
//! 4. **`epoch` 原样带回**：`intercept` 拿到的 `epoch` 就是 `Effect::Compact`
//!    自己带的那个，原样记进 [`crate::compact_slot::CompactSlots`]；真正的闸在
//!    `Session::step` 入口（105 已落地），这里只负责别把它弄丢。
//!
//! # 子怎么读到「父的历史」：一条纯文本 user 消息，不是新读口
//!
//! 复用「任务文本是子的第一条 user 消息」这条既有约定（`crate::subagent` 模块
//! 文档 §「M3 v1 的固定模板」）——没有为摘要开一条新的跨 agent 读接口。红线 10
//! 那句「摘要子 agent 读父的历史是向上读，允许」，物理实现就是这一条消息：父在
//! [`intercept`] 里把 `[0, upto)` 里的内容一次性渲染成文本交给子，子 agent 自己
//! 再也不需要（也没有 API）反过来翻父的状态。
//!
//! # 摘要吃的是原始历史，不是投影之后的占位符
//!
//! [`compose_task`] 读的是 `Session::messages_of` 的原始消息，不经过第 2 档
//! （`clear_tool_results`）的 `SendPlan` 投影。第 3 档是在第 2 档已经清光了还不够
//! 时才触发的最后一招（096 决策记录），这一段历史此刻在父的 prompt 里可能已经是
//! 占位符——但摘要要的是「这段到底发生了什么」，喂给它一份已经被占位符替换过的
//! 二手材料只会让摘要更空洞。多花的这份长度由 [`crate::ctx::RunnerCtx::
//! compaction_execution_profile`] 配一个便宜模型去吃（096 决策记录：「摘要那次
//! 模型调用本身可以走便宜模型」）。

use std::sync::Arc;

use agent_core::{AgentId, ChildConfig, ContentBlock, Epoch, Event, Message, Role, Session};

use crate::compact_slot::CompactSlots;
use crate::ctx::RunnerCtx;
use crate::dispatch::Dispatched;
use crate::persist;

/// 摘要指令：产品判断，写死在宿主侧（契约 1）。语气和要点是刻意选的——
/// 「只留三类东西」是为了让摘要落在「后续对话接得上」这个目标上，不是一份
/// 面面俱到的会议纪要。
const SUMMARY_INSTRUCTIONS: &str = "\
你的唯一任务是把下面这段对话历史压缩成一份摘要，交给接下来的对话继续用。\n\
- 只留三类东西：用户交代过的目标或约束、已经确认的结论或事实、还没解决的问题。\n\
- 直接写内容本身，不要写「用户说了」「助手回复了」这类转述腔。\n\
- 不要编造这段历史里没有的信息。\n\
- 直接给摘要正文，不要加任何开场白或结束语。\n\
\n\
对话历史：\n";

/// 起飞：把 `Effect::Compact` 变成一次真实的子 agent spawn。
///
/// `agent` 是被压缩历史的归属方（`Effect::Compact` 的 `agent` 字段），也是回执
/// 事件（`Event::CompactDone`/`CompactFailed`）最终要喂给的那个 agent——**不是**
/// 摘要子 agent 自己。
pub(crate) fn intercept(
    session: &mut Session,
    ctx: &mut RunnerCtx,
    compactions: &mut CompactSlots,
    agent: AgentId,
    upto: usize,
    epoch: Epoch,
) -> Dispatched {
    let child_config = ChildConfig {
        execution_profile: ctx.compaction_execution_profile.clone(),
        ..ChildConfig::default()
    };
    let child = match session.spawn_child(&agent, child_config, None) {
        Ok(child) => child,
        Err(_refused) => return Dispatched::Event(Event::CompactFailed { agent, epoch }),
    };
    // `spawn_child` 是一条命令，落了一条 Entry——跟 `spawn_tool::intercept` 同一条
    // 理由，得立刻转发进持久化后端，否则进程在摘要子 agent 干活期间崩溃，恢复出来
    // 的会话里会有一个「有工作痕迹但没有出生记录」的 agent。
    persist::sync(ctx, session);

    let history = session.messages_of(&agent);
    let task = compose_task(history.iter().take(upto));

    // `upto` 一起记下：回执事件里没有它（105 定死的形状），而 107 的
    // `apply_summary` 要它——这张表因此是那条硬契约唯一的物理载体
    // （`crate::compact_slot` 模块文档）。
    compactions.record(child.clone(), agent, epoch, upto);
    // 子 agent 怎么开工没有第二份代码：任务文本经 `Event::UserInput` 走正门，
    // 跟 `spawn_tool::intercept` 逐字同一条路（`crate::subagent` 模块文档
    // 「子 agent 怎么开始干活因此没有专门的代码路径」）。
    Dispatched::Event(Event::UserInput {
        agent: child,
        text: Arc::from(task),
    })
}

/// 摘要子 agent 的第一条（也是唯一一条）user 消息：固定指令 + 那段历史的纯文本
/// 渲染。
fn compose_task<'a>(messages: impl Iterator<Item = &'a Message>) -> String {
    let mut out = String::from(SUMMARY_INSTRUCTIONS);
    for message in messages {
        render_message(&mut out, message);
    }
    out
}

/// 渲染一条消息。**`Thinking` 块不进摘要材料**——跟
/// `crate::child_outcome::final_text` 同一个判断：那是模型的思考过程，要不要采信
/// 是 adapter 的判断，不该由我们替摘要子 agent 决定它算不算「发生过的事」。
/// `ToolUse`/`ToolResult` 保留：它们是这段历史里真正的「发生了什么」，压缩掉的话
/// 摘要会变成一份空洞的对话腔调复述。
fn render_message(out: &mut String, message: &Message) {
    let speaker = match message.role {
        Role::User => "用户",
        Role::Assistant => "助手",
    };
    for block in &message.blocks {
        match block {
            ContentBlock::Text(text) => {
                out.push_str(speaker);
                out.push('：');
                out.push_str(text);
                out.push('\n');
            }
            ContentBlock::ToolUse { name, input, .. } => {
                out.push_str(speaker);
                out.push_str("调用了工具 ");
                out.push_str(name);
                out.push_str("，参数：");
                out.push_str(&input.to_string());
                out.push('\n');
            }
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                out.push_str(if *is_error {
                    "工具报错："
                } else {
                    "工具结果："
                });
                out.push_str(content);
                out.push('\n');
            }
            ContentBlock::Thinking(_) => {}
        }
    }
}

#[cfg(test)]
#[path = "compact_spawn_tests.rs"]
mod tests;
