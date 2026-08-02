//! 工具表：宿主侧持有的「工具在哪跑、可逆性怎么定」（002 合并记录：core 没有
//! 这份数据，`ExecuteTool` 快照的构造点在宿主/command 层——M1 这里就是那个
//! 宿主）。
//!
//! `agent_core::ToolSpec` 只有喂给模型的三个字段（name/description/schema），
//! 没有 `Location`/`Reversibility`——那两个维度是 router/undo 用的，`agent-tools`
//! 也没暴露它们（013 的 `ToolExecutor` 只按全名分发，不声明位置/可逆性）。这张表
//! 补的就是这一格。

use std::sync::Arc;

use agent_core::{AgentLimits, Location, Reversibility, ToolCallRequest, ToolSpec};
use serde_json::Value;

use crate::spawn_tool::{SPAWN_TOOL, spawn_spec};

/// 会话期间不变的工具表：喂模型的声明 + 判 `location`/`reversibility` 用的
/// 名字规则。
pub struct ToolTable {
    specs: Vec<ToolSpec>,
}

impl ToolTable {
    /// 013 的内置工具集：`srv:fs/read`、`srv:fs/list`，服务端本地、纯读。
    pub fn builtin() -> Self {
        ToolTable { specs: agent_tools::builtin_specs() }
    }

    /// 027 开闸：内置只读集 + `srv:shell/exec`（020 声明、`agent-tools` 的
    /// `ToolExecutor` 早已支持分发，唯独没接进任何工具表——020 的范围裁决要求
    /// 「屏障 UI 齐了才许打开」，027 就是那个集成 issue）。追加在末尾而不是
    /// 插进 `builtin()` 内部：`builtin_specs()` 的顺序是 013 钉死的既有契约，
    /// 这里只加不改。
    pub fn with_shell() -> Self {
        let mut specs = agent_tools::builtin_specs();
        specs.push(agent_tools::shell_spec());
        ToolTable { specs }
    }

    /// 029 开闸：追加 `srv:agent/spawn`，宿主从此允许模型分解任务（决策 20）。
    ///
    /// **`limits` 必须跟 `Session` 手上那份是同一组数**（`Session::agent_limits`，
    /// 默认都是 [`AgentLimits::default`]）：这里的数字只进工具描述给模型看，真正
    /// 拦人的是 `Session::spawn_child` 里那两道闸。两边不一致不会出错，只会让模型
    /// 收到一句跟描述对不上的拒绝——所以宿主要么两边都用默认值，要么两边传同一个
    /// 值。数字进描述而不是让模型试出来，是为了省掉大部分「试→被拒→重试」的往返。
    ///
    /// 追加在末尾而不是插进 `builtin()` 内部：`builtin_specs()` 的顺序是 013 钉死
    /// 的既有契约，工具表在 prompt 最前面（红线 11），只加不改。
    pub fn with_spawn(mut self, limits: AgentLimits) -> Self {
        self.specs.push(spawn_spec(limits));
        self
    }

    /// 喂给 `Ingredients::tools` 的那张表，顺序原样保留（红线 11）。
    pub fn specs(&self) -> &[ToolSpec] {
        &self.specs
    }

    /// 这张表里有这个工具吗。
    ///
    /// 唯一的用处是 spawn 的截获闸（`crate::dispatch`）：**宿主没声明就不截获**，
    /// 模型凭空猜出来的 `srv:agent/spawn` 跟别的不存在的工具走同一条路
    /// （`unknown_tool`），而不是在一个没打算开子 agent 的宿主上凭空长出一棵树。
    pub fn declares(&self, tool: &str) -> bool {
        self.specs.iter().any(|spec| &*spec.name == tool)
    }

    /// 按全名 + 这次调用的 `input` 构造一次调用的「发起时快照」。
    ///
    /// `location` 从名字前缀机械解析（docs/TOOLS.md 的命名约定：`srv:`/`web:`/
    /// `desk:`）；`reversibility` **拿不准就 `Irreversible`**
    /// （value/tool.rs 的判据：判错代价不对称，保守值必须是默认值）——M1 的
    /// 两个内置工具都是已知的纯读，显式列出，其余一律走保守默认，不臆造
    /// `Pure`。
    pub fn snapshot(&self, tool: &str, input: Arc<Value>) -> ToolCallRequest {
        ToolCallRequest {
            tool: Arc::from(tool),
            input,
            location: location_of(tool),
            reversibility: reversibility_of(tool),
        }
    }
}

fn location_of(tool: &str) -> Location {
    match tool.split_once(':').map(|(prefix, _)| prefix) {
        Some("web") => Location::Web,
        Some("desk") => Location::Desktop,
        // `srv` 或者压根没有认得出的前缀：M1 没有 router，落进这个分支的只有
        // 013 的内置工具，全部是 `srv:` 前缀——保守当作本地服务端处理。
        _ => Location::Server,
    }
}

fn reversibility_of(tool: &str) -> Reversibility {
    match tool {
        "srv:fs/read" | "srv:fs/list" => Reversibility::Pure,
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
        _ => Reversibility::Irreversible,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_specs_are_exposed_in_order() {
        let table = ToolTable::builtin();
        let names: Vec<&str> = table.specs().iter().map(|s| &*s.name).collect();
        assert_eq!(names, vec!["srv:fs/read", "srv:fs/list"]);
    }

    #[test]
    fn known_builtin_tools_are_pure_reads() {
        let table = ToolTable::builtin();
        let snap = table.snapshot("srv:fs/read", Arc::new(Value::Null));
        assert_eq!(snap.location, Location::Server);
        assert_eq!(snap.reversibility, Reversibility::Pure);
    }

    /// 拿不准的工具名：位置按前缀猜，可逆性保守落 `Irreversible`——
    /// 判错成 `Pure` 的代价（重复扣款）比判错成 `Irreversible`（多问一次）大。
    #[test]
    fn unknown_tool_defaults_to_irreversible() {
        let table = ToolTable::builtin();
        let snap = table.snapshot("web:browser/click", Arc::new(Value::Null));
        assert_eq!(snap.location, Location::Web);
        assert_eq!(snap.reversibility, Reversibility::Irreversible);
    }

    /// 027 开闸：`with_shell()` 在内置只读集后面追加 `srv:shell/exec`，
    /// 且它落 `Irreversible`（走的是保守默认分支，不需要额外列进
    /// `reversibility_of` 的已知表——`unknown_tool_defaults_to_irreversible`
    /// 已经证明这条分支的判据，这里只需确认它真的在表里）。
    #[test]
    fn with_shell_appends_shell_exec_after_the_read_only_builtins_and_it_is_irreversible() {
        let table = ToolTable::with_shell();
        let names: Vec<&str> = table.specs().iter().map(|s| &*s.name).collect();
        assert_eq!(names, vec!["srv:fs/read", "srv:fs/list", "srv:shell/exec"]);

        let snap = table.snapshot("srv:shell/exec", Arc::new(Value::Null));
        assert_eq!(snap.location, Location::Server);
        assert_eq!(snap.reversibility, Reversibility::Irreversible);
    }

    /// 029 开闸：`with_spawn` 同样只追加在末尾，且 spawn 是 `Reversible`
    /// （补偿 = `despawn_child`，理由见 `reversibility_of` 的注释）——它**不是**
    /// 那个保守默认分支的产物，所以这里两件事都得断言。
    #[test]
    fn with_spawn_appends_the_spawn_tool_and_it_is_reversible() {
        let table = ToolTable::with_shell().with_spawn(AgentLimits::default());
        let names: Vec<&str> = table.specs().iter().map(|s| &*s.name).collect();
        assert_eq!(names, vec!["srv:fs/read", "srv:fs/list", "srv:shell/exec", "srv:agent/spawn"]);

        let snap = table.snapshot(SPAWN_TOOL, Arc::new(Value::Null));
        assert_eq!(snap.location, Location::Server);
        assert_eq!(snap.reversibility, Reversibility::Reversible);
    }

    /// 截获闸的输入端：宿主没开子 agent，这个名字就跟别的不存在的工具一样。
    #[test]
    fn a_table_without_spawn_does_not_declare_it() {
        assert!(!ToolTable::builtin().declares(SPAWN_TOOL));
        assert!(ToolTable::builtin().with_spawn(AgentLimits::default()).declares(SPAWN_TOOL));
    }

    /// 上限进描述是给模型看的（029：「描述写给模型看」），换一组数就该换一份
    /// 描述——不然模型读到的上限跟真正拦它的那两道闸对不上。
    #[test]
    fn the_declared_limits_follow_the_limits_that_are_actually_enforced() {
        let default = ToolTable::builtin().with_spawn(AgentLimits::default());
        let tighter = ToolTable::builtin().with_spawn(AgentLimits { max_depth: 1, max_children: 2 });
        let text = |t: &ToolTable| t.specs().last().unwrap().description.to_string();
        assert!(text(&default).contains('8'));
        assert!(text(&tighter).contains('2') && !text(&tighter).contains('8'));
    }
}
