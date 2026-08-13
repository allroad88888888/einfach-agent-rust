//! 148：一个 Rust 扩展的**交付物形状**（决策 29 的落点，接缝文档
//! [`docs/EXTENSIONS.md`](../../../docs/EXTENSIONS.md)）。
//!
//! 这个文件只管**一包长什么样**（含它自己的名字规则）；它怎么分两阶段装进一个
//! 会话，全在 [`crate::tool_table`] 的 `extension` 子模块（`tool_table_extension.rs`）。
//!
//! # 一包 = 宿主的一行授权
//!
//! 扩展能带进会话的东西只有两类：**截获式工具**（拿 `Session` 手套读写状态的唯一
//! 正门，146 的 [`SessionToolFn`]）和 **timed 工具**（开局/收尾 runtime 自动跑一次
//! 的钩子，133 的 [`TimedRun`]）。打成一个结构体之后，「装不装这个扩展」对宿主
//! 才真的是一行——而不是「记得把 spec 加进表、记得把闭包注册进 ctx、记得把可逆性
//! 填对」这三件必须同时记住、漏一件不报错的事。
//!
//! # 三元成对：spec、可逆性、执行体一起进
//!
//! [`ExtensionPack::with_tool`] 一次收三个，中间没有「先声明、以后再补执行体」的
//! 状态。这是 147 的教训直接写进类型：`dispatch.rs` 里那四条手工截获的**声明**
//! （`ToolTable::with_*` 的 `push_spec`）和**执行路径**（`if` 链）分住两个文件，
//! 改一半而另一半没跟上不会报错——只会让模型看见一个永远落 `unknown_tool` 的名字，
//! 或者让一个没人声明的名字被偷偷执行（146 那三道闸挡的正是这两件事）。三元一格，
//! 「半开」在这个类型里表达不出来。
//!
//! **可逆性没有缺省**：不是「不填就 `Irreversible`」，是**「填不填」这个选择不存在**
//! ——它是 [`ExtensionPack::with_tool`] 的第二个位置参数，少给一个不编译。148 在
//! 这点上比 issue 原文（「缺省 `Irreversible`」）严一档，理由是缺省值等于告诉作者
//! 「这件事可以不想」，而它恰恰是 `/undo` 撞上这个工具时停不停的唯一依据。怎么判
//! 见 `docs/TOOLS.md` §可逆性：**拿不准就 `Irreversible`**，判错的代价不对称
//! （判宽了只是多问一句，判窄了是真的放过一次删除）；给 `Pure` 的举证责任在包
//! 作者身上——纯读、不落 entry、没有需要补偿的动作，三条都成立才是 `Pure`。
//!
//! # 名字：`ext:<pack>/<tool>` 强制，不是建议
//!
//! 069 的红利照抄 MCP：**能靠命名让撞名不可能发生，就不要去写策略**。包名进名字
//! 之后，两个扩展之间、扩展与内置五档之间、扩展与 M10 注入的 `web:`/`desk:` 之间
//! 结构上撞不了；`location_of` 也因此能一眼把 `ext:` 判成 `Server`（扩展是编译期
//! 依赖，跟内置工具跑在同一个进程里，见 `tool_table_names.rs` 那一条）。
//!
//! 冒用别人的前缀比裸名更该拦：一个叫 `web:foo/bar` 的扩展工具会被 `location_of`
//! 判成远端，dispatch 于是把它送去等一个永远不会来的宿主回传（`web:` 是 M10 强制
//! 给注入工具的前缀，没有任何人会认领它）——**不报错，只是这个工具永远调不通**，
//! 正是 `docs/TOOLS.md` §命名空间点名过的那类静默失效。
//!
//! # 违规的粒度：丢**这一条**，不丢整包
//!
//! 判据同 [`ToolTable::push_spec`](crate::ToolTable) / `with_timed`：作者是程序员
//! （扩展是编译期依赖，不是运行时数据），`debug_assert!` 在他自己那次
//! `cargo test` 里就炸出来，点名是哪个包的哪个名字；release 只丢**这一条**，
//! 包里其余条目照常装。
//!
//! 为什么不是「整包丢弃」：丢整包会让一个 hook 名字的笔误顺手关掉同一包里那个
//! 完全合法的读工具，故障点离肇事的那一行更远；而「丢这一条」跟表那两个入口逐字
//! 同一句话，读者不用记第二套规矩。安全上两者等价——被丢的那条从此既不进 prompt
//! 也不进任何执行路径，冒用前缀想骗到的那次派发根本不会发生。

use std::sync::Arc;

use agent_core::{Reversibility, ToolSpec};

// `SessionToolFn` 走 crate 根的再导出而不是它此刻的定义模块：146/147 期间那个类型
// 的家还在挪（`intercept_registry` → 更专门的文件），根导出是它稳定的公开路径。
use crate::SessionToolFn;
use crate::tool_table::{CallTiming, TimedRun};

/// `ext:<pack>/<tool>` 的固定头。
const EXT_PREFIX: &str = "ext:";

/// 一个 Rust 扩展要带进会话的全部东西（决策 29 的交付物）。
///
/// 字段全私有、只能经 [`ExtensionPack::with_tool`] / [`ExtensionPack::with_timed`]
/// 往里加：那两个方法是名字规则唯一的检查点，公开字段等于把这道闸拆了（而且
/// 拆掉之后没有任何地方还能在「哪一行加错了」这个粒度上报错）。
///
/// 装配见 [`ToolTable::with_extension`](crate::ToolTable::with_extension)——
/// 表半边与 ctx 半边分两阶段，那边的模块文档解释为什么以及怎么防「只装一半」。
pub struct ExtensionPack {
    name: Arc<str>,
    /// 截获式工具：声明、可逆性、执行体三元成对。`Vec` 而不是任何 map——
    /// 顺序就是进 prompt 的顺序，由作者的代码写死（红线 11）。
    tools: InterceptEntries,
    /// timed 工具：声明、时机、执行体三元成对。**不进 prompt**（timed 区对
    /// `specs()`/`declares()` 不可见，133），但顺序仍然是执行顺序。
    timed: TimedEntries,
}

/// 截获式工具的三元组序列。**只是给 [`ExtensionPack::into_parts`] 的返回类型起个名字**——
/// 那个返回值是「名字 + 两条序列」的三元组，写平了 clippy 的 `type_complexity` 会红，
/// 而拆成结构体又会给一个只在装配那一刻活着的中间物起名。语义一个字节没变。
type InterceptEntries = Vec<(ToolSpec, Reversibility, SessionToolFn)>;

/// timed 工具的三元组序列。理由同 [`InterceptEntries`]。
type TimedEntries = Vec<(ToolSpec, CallTiming, TimedRun)>;

impl ExtensionPack {
    /// 开一个空包。`name` 进每个工具的全名（`ext:<name>/<tool>`），也是日志和
    /// 宿主授权面上认这个扩展的那个词。
    ///
    /// 包名本身不能为空、不能含 `:` 或 `/`——含了就会让 `ext:a/b` 这个名字同时
    /// 能被包 `a/b` 和包 `a`（工具 `b/…`）声称，命名空间的那份红利当场作废。
    /// `debug_assert!` 点名；release 原样留着（**绝不 sanitize**，`docs/TOOLS.md`
    /// §撞名：悄悄把名字洗一遍会让两个本来不同的声明撞成同一个），此时每条工具
    /// 名仍要逐字匹配这个畸形前缀才进得来，跑不出 `ext:` 这个命名空间。
    pub fn new(name: impl Into<Arc<str>>) -> Self {
        let name = name.into();
        debug_assert!(
            !name.is_empty() && !name.contains(':') && !name.contains('/'),
            "扩展包名 `{name}` 不合法：非空、且不含 `:` 与 `/`（它要进 `ext:<pack>/<tool>`）"
        );
        ExtensionPack {
            name,
            tools: Vec::new(),
            timed: Vec::new(),
        }
    }

    /// 加一条**截获式工具**：模型看得见的声明、`/undo` 用的可逆性、拿 `Session`
    /// 手套干活的执行体，一次给全。
    ///
    /// `run` 收的是 146 的公开层 [`SessionToolFn`]（`Box<dyn Fn(&mut Session,
    /// &AgentId, &Value) -> Result<Arc<str>, Arc<str>>>`），不是 `impl Fn`：跟
    /// `RunnerCtx::register_session_tool` / `ToolTable::with_timed` 两个既有注册
    /// 入口同一个形状——同一件事只留一种写法，省掉「装箱在调用方还是被调方」这个
    /// 每次都要重新回答的问题。
    ///
    /// 闭包里的读写纪律（后代收窄、只走 command 面）机制不强制，见
    /// [`SessionToolFn`] 的文档与 `docs/EXTENSIONS.md` §正门。
    pub fn with_tool(
        mut self,
        spec: ToolSpec,
        reversibility: Reversibility,
        run: SessionToolFn,
    ) -> Self {
        if self.accepts(&spec.name, "截获式工具") {
            self.tools.push((spec, reversibility, run));
        }
        self
    }

    /// 加一条 **timed 工具**（开局 / 轮末的钩子，133）。名字同样吃 `ext:` 前缀
    /// 强制——它虽然不进 prompt，却跟 specs 区共用同一个名字空间
    /// （`with_timed` 的撞名双向查），一条裸名 timed 钩子照样能把一个内置工具名
    /// 占掉。
    ///
    /// 时机语义、失败怎么办、结果去哪儿全在 [`CallTiming`] 与
    /// [`crate::tool_table`] 的 `timed` 子模块：`SessionStart` 全有或全无，
    /// `TurnEnd` 结果丢弃、失败只记日志。
    pub fn with_timed(mut self, spec: ToolSpec, timing: CallTiming, run: TimedRun) -> Self {
        if self.accepts(&spec.name, "timed 工具") {
            self.timed.push((spec, timing, run));
        }
        self
    }

    /// 这个包叫什么（宿主授权面 / 日志用）。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 名字规则这道闸。`kind` 只进错误文案，用来说清是哪一类条目违规。
    fn accepts(&self, name: &str, kind: &str) -> bool {
        let ok = belongs_to(&self.name, name);
        debug_assert!(
            ok,
            "扩展包 `{}` 的{kind} `{name}` 不叫 `ext:{}/<tool>`：\
             扩展工具名前缀强制（决策 29 / 069 的命名红利），这一条整条丢弃",
            self.name, self.name
        );
        ok
    }

    /// 装配用：把三样东西拆出来交给 `ToolTable::with_extension`。
    ///
    /// `pub(crate)` 且**消费自身**——包一旦拆开就没有「原样再装一次」的形态，
    /// 两个阶段拿到的必然是同一个实例拆出来的两半（那边模块文档「同一个包实例」
    /// 这句承诺的机制就是这一行）。
    pub(crate) fn into_parts(self) -> (Arc<str>, InterceptEntries, TimedEntries) {
        (self.name, self.tools, self.timed)
    }
}

/// `name` 是不是 `pack` 这个包的合法工具名：`ext:<pack>/` 开头且尾巴非空。
///
/// 逐段 `strip_prefix` 而不是 `starts_with(&format!(...))`：包 `demo` 与名字
/// `ext:demo2/x` 在「拼串再比」的写法下也不会误判，但逐段剥能顺手把「尾巴为空」
/// （`ext:demo/`）这个畸形挡掉，而且不为每次检查分配一个临时 `String`。
fn belongs_to(pack: &str, name: &str) -> bool {
    name.strip_prefix(EXT_PREFIX)
        .and_then(|rest| rest.strip_prefix(pack))
        .and_then(|rest| rest.strip_prefix('/'))
        .is_some_and(|leaf| !leaf.is_empty())
}

#[cfg(test)]
#[path = "extension_pack_tests.rs"]
mod tests;
