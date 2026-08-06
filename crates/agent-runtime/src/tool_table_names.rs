//! **名字规则**：一个工具的全名怎么机械地推出它那两个**不进 prompt** 的维度。
//!
//! 从 `tool_table.rs` 分出来的一件事（076 的红线 9 拆分）。`ToolSpec` 只有喂给模型
//! 的三个字段（name/description/schema），`Location`（这个调用派到哪儿跑）和
//! `Reversibility`（`/undo` 撞上它要不要停下来问）都不在里面——它们由这两个函数
//! 从**名字**推出来。工具表的五档装配、`snapshot` 的三级优先级、宿主注入与 skill
//! 各自那件事，都留在各自的文件里。
//!
//! # 为什么是「按名字推」而不是「查一张表」
//!
//! 这是 `docs/TOOLS.md` 的命名约定在代码里唯一的落点：`srv:` 在服务端进程内跑、
//! `web:`/`desk:` 派回宿主、`mcp:` 是宿主本地起子进程往返。前缀既是给人看的分类，
//! 也是路由判据——两者是同一件事，多一张表就多一处会跟名字对不上的地方。
//!
//! **例外都写在 [`ToolTable::snapshot`](super::ToolTable::snapshot) 那三级优先级里，
//! 不在这里**：宿主注入的可逆性按表查（062）、MCP 的可逆性按 server 的 `readOnlyHint`
//! 查（041/042）。这个文件只管「没有任何人替它说话时，名字告诉我们什么」。
//!
//! # `reversibility_of` 拿不准就 `Irreversible`
//!
//! 判错的代价不对称（`value/tool.rs` 的判据）：把不可逆的判成 `Pure`，`/undo` 就
//! 白白放过一次真的删了东西的调用；反过来只是多问一句。所以保守值必须是**默认值**，
//! 已知的纯读/可补偿的显式列出，其余一律落进 `_ =>` 那一支，不臆造 `Pure`。

use agent_core::vision::VISION_INSPECT_TOOL;
use agent_core::{Location, Reversibility};

use crate::collect_tool::COLLECT_TOOL;
use crate::skill::{SKILL_ACTIVATE, SKILL_DEACTIVATE};
use crate::spawn_tool::SPAWN_TOOL;
use crate::status_tool::STATUS_TOOL;

pub(super) fn location_of(tool: &str) -> Location {
    if matches!(tool, "ask_user_question" | "browser_action" | "save_file") {
        return Location::Web;
    }
    match tool.split_once(':').map(|(prefix, _)| prefix) {
        Some("web") => Location::Web,
        Some("desk") => Location::Desktop,
        // MCP 调用在**宿主本地**执行（`crate::mcp_call` 起子进程往返），对 loop 而言
        // 结果经泵落地、不需要远端回传——所以是 `Server`，不是 remote（043）。
        Some("mcp") => Location::Server,
        // `srv` 或者压根没有认得出的前缀：M1 没有 router，落进这个分支的只有
        // 013 的内置工具，全部是 `srv:` 前缀——保守当作本地服务端处理。
        _ => Location::Server,
    }
}

pub(super) fn reversibility_of(tool: &str) -> Reversibility {
    match tool {
        "srv:fs/read"
        | "srv:fs/list"
        | "read_file"
        | "list_files"
        | "search_files"
        | "rg_search"
        | "find_test_lint_commands"
        | "git_diff_review"
        | "ask_user_question" => Reversibility::Pure,
        // spawn 的补偿动作是 `despawn_child`（028 已经实现，019 三约束逐条走完）
        // ——**有明确且可靠的补偿动作**正是 `Reversible` 的定义。
        //
        // 「可子 agent 会去干不可逆的事啊」：那些事各自带自己的屏障位——子 agent
        // 跑 `shell/exec` 时，记录那条结果的 entry 就是 `barrier: true`，而它跟
        // 父的 spawn 那条 entry 在**同一条日志、同一个 turn_id** 上（决策 5）。
        // undo 往回走会先撞上子 agent 那条屏障停下来问，轮不到 spawn 这条。
        // 组合因此天然成立，不需要 spawn 自己保守成 `Irreversible`——那样反而会
        // 让「拆了任务的那一轮」一律撤不掉，哪怕子 agent 只读了两个文件。
        SPAWN_TOOL => Reversibility::Reversible,
        // status 是**纯读**：一次 `Session::agent_tree()` 派生读，不写任何
        // primitive、不落 entry、没有需要补偿的动作——`Pure` 的定义本身，
        // 跟 `srv:fs/read` 同一格。于是它不进 `mark_irreversible`，日志上不留
        // 屏障位，`/undo` 路过它时不会停下来问（问了也没有东西可撤）。
        STATUS_TOOL => Reversibility::Pure,
        // collect 同理是**纯读**：读一份子 agent 已经产出的结果（经宿主 harvest
        // 回写），不写任何 primitive、不落 entry、没有需要补偿的动作。
        //
        // 「可子 agent 干过不可逆的事啊」——那些事各自带**它自己的**屏障位（子跑
        // `shell/exec` 时记录结果那条 entry 就是 `barrier: true`），跟这条 collect
        // 在同一条日志、同一个 turn_id 上。undo 往回走先撞子那条屏障停下来问，
        // 轮不到 collect。跟 spawn 判 `Reversible` 是同一套账（见上一条注释）。
        COLLECT_TOOL => Reversibility::Pure,
        // 专用视觉门面只读取本轮已注册的附件并返回观察文本；provider 费用不改变
        // agent timeline 的可逆性，跟其它纯读工具一样不落 undo 屏障。
        VISION_INSPECT_TOOL => Reversibility::Pure,
        // skill 激活/停用的补偿动作就是彼此（`srv:skill/deactivate` / re-activate），
        // 而且激活走 command 层、journaled——**有明确且可靠的补偿动作** = `Reversible`
        // 的定义。它们走 dispatch 截获、不进 `mark_irreversible`，所以不在日志上留
        // 屏障位：`/undo` 连激活一起退掉是白拿的。
        SKILL_ACTIVATE | SKILL_DEACTIVATE => Reversibility::Reversible,
        _ => Reversibility::Irreversible,
    }
}
