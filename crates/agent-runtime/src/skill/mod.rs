//! skill 装载：宿主侧的 `SkillRegistry`——从磁盘 `SKILL.md` 或宿主声明把 skill
//! 读进来，展开成「常驻索引」（138/139，进 system 前缀）与「按需正文」
//! （137，`srv:skill/read`）。
//!
//! # 141：激活式注入已删
//!
//! 039（决策 21）曾经的形状是「模型经 `srv:skill/activate` 激活 → 正文进料单的
//! 正文段、携带的工具进料单的中途工具段」。决策 27（M15）把这条路整个换掉：
//! 正文改成按需 `read`（137），常驻索引改成开局工具（138/139）。
//! [141](../../../../docs/issues/141-remove-activation-subsystem.md) 删掉了
//! 激活/停用工具、那条把激活集展开成两份注入料的方法、以及只为「解析一个已
//! 激活 skill 携带的远端工具」而存在的执行授权机制（`SkillSource`）——
//! `capabilities.skills[].tools` 非空已经在 server 侧整份 400（140），skill 携带
//! 工具在 v1 没有任何时机能生效，那套授权代码留着只是死代码。
//!
//! # 边界：registry 是宿主的，不是 core 的
//!
//! `agent-core` 只认识「哪些 skill id 曾经被激活过」（`Slot::SkillsActive`，槽位
//! 留壳、无写入点，见 `agent-core` 的 `command/skill.rs`）。**内容活在这里**——
//! store 之外、可以做 IO（红线 7 只约束 core/store）。
//!
//! # 子模块
//!
//! - [`yaml`]：SKILL.md frontmatter 的缩进式 YAML 子集解析（无外部依赖）。
//! - [`load`]：目录遍历 + frontmatter/正文切分 + 建 [`Skill`]。
//! - [`read`]（137）/ [`index`]（138）：正文按需读 + 索引文本，装配见 `tool_table_skill.rs`。

mod index;
mod load;
mod read;
mod yaml;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use agent_core::SkillId;

pub use index::index_spec;
pub use load::SkillLoadError;
pub(crate) use read::intercept as read_intercept;
pub use read::{SKILL_READ, read_spec};

/// 一个装载进来的 skill：id（= frontmatter 的 `name`）、进索引的那行描述、正文、
/// 以及历史遗留的可选携带工具（141 起没有任何路径会执行它，字段仅供数据往返）。
struct Skill {
    id: SkillId,
    description: Arc<str>,
    body: Arc<str>,
    /// **141 之后是纯数据**：装载/声明时照旧解析进来，但没有任何执行路径会用
    /// 到它——server 侧已经在声明这一步拒绝非空的 skill tools（140，决策 27），
    /// 磁盘 skill 的这个字段也从未有过执行授权（139 之前就只影响 late_tools
    /// 的可见性）。留着字段是为了 `load.rs`/`yaml.rs` 的 frontmatter 解析不用
    /// 跟着砍一刀，且不改变 `agent_core::HostSkill` 的既有形状。
    #[allow(dead_code, reason = "141 之后是纯数据，没有执行路径读它")]
    tools: Vec<agent_core::ToolSpec>,
    /// frontmatter 可选的 `hidden`（142）：`true` 不进索引，但 `body_of`/
    /// `srv:skill/read` 照常可读——只挡「发现」，不挡「读」。
    hidden: bool,
}

/// 宿主持有的 skill 目录索引。**用 `BTreeMap` 不是 `HashMap`（红线 11）**：索引
/// 内容会进 prompt，迭代顺序必须逐字节确定。
///
/// 被 `ToolTable::with_skills` 拥有（跟工具表同寿命），供 dispatch 在整段
/// `run_turn` 期间随时查——不是只在建表那一刻用一次。
pub struct SkillRegistry {
    skills: BTreeMap<Arc<str>, Skill>,
}

impl SkillRegistry {
    /// 从若干来源目录装载（内置 + 项目 `./skills/`……）。**合并：后一个目录里同名
    /// skill 整体覆盖前一个**——069 拍板的**有意例外**（docs/TOOLS.md §撞名）：目录
    /// 顺序是部署者显式排的（内置 → 项目 → 用户），「后面盖前面」正是覆盖机制本身
    /// 的用途，不是没想清楚该要哪个。它跟 `capabilities` 的「重名一律拒绝」不矛盾：
    /// 那边两个候选出自同一份声明、同一个作者，没有先后可言，server 替它选一个就是
    /// 把问题推到运行时；这边先后本身就是作者给的信息。
    ///
    /// 覆盖是**整体替换**不是字段级 merge（没有「一半 A 一半 B」的混血），所以合并完
    /// 每个 id 恰好一份——069 那条红线「撞名不许留到 prompt 里」在这条路上是白拿的。
    /// **工具表不是这套规则**（它今天压根不检测重名，069 §拍板 D 定的修法是「后来的
    /// 整条不进表」，实现排在 062 之后）。
    ///
    /// 不存在的目录**跳过、不报错**——宿主指向一个还没建的 `./skills/` 是常态。
    pub fn load(dirs: &[PathBuf]) -> Result<Self, SkillLoadError> {
        let mut skills = BTreeMap::new();
        for dir in dirs {
            load::load_dir(dir, &mut skills)?;
        }
        Ok(SkillRegistry { skills })
    }

    /// 空 registry（宿主没开 skill 时的占位；`ToolTable` 的默认值）。
    pub fn empty() -> Self {
        SkillRegistry {
            skills: BTreeMap::new(),
        }
    }

    /// 064：**宿主建会话时声明的 skill** 进这一个会话的 registry
    /// （`docs/HOST-CAPABILITIES.md` §八）。声明经 `agent_core::HostSkill` 进来——
    /// 那是它落进 store（`Slot::HostSkills`，journaled）再回放出来的同一个形状，
    /// 所以「新建」和「恢复」两条路喂给 registry 的是**同一份数据**。
    ///
    /// # 为什么是构造器，不是能接在 [`load`](SkillRegistry::load) 后面的 builder
    ///
    /// 069 §拍板「顺带定死 064 第 3 条」：**server 形态不从磁盘 `./skills/` 装载**。
    /// 两个来源合流会造出「同一份请求在不同部署上行为不同」的面，而且 073 之后
    /// 宿主声明是**会话状态**（恢复时逐字节复刻），磁盘上那份不是——部署者改一下
    /// `./skills/` 就能悄悄改写一段历史对话该长什么样，正好是 073 刚堵上的那个洞。
    ///
    /// 写成构造器而不是 `self` builder，是把这条决定钉进类型：想合流的人必须**显式**
    /// 加一条合并路径（061 同一处闸：宿主声明的 id 撞上磁盘已装载的 id → 400）。
    ///
    /// **141**：同一份声明内部撞 id 已经在 061 这一层拒绝（`DuplicateSkill` →
    /// 400），后到的整条覆盖前一条即可（跟 [`load`](SkillRegistry::load) 目录合并
    /// 同一条「后来居上」语义）——不再需要专门标一个 `InvalidHost` 让撞名的 skill
    /// 连工具都不能执行：那套授权判定本身随 `active_host_tool_request` 一起删了。
    pub fn from_host_skills(skills: Vec<agent_core::HostSkill>) -> Self {
        let mut registry = BTreeMap::new();
        for declared in skills {
            let skill = Skill {
                id: declared.id,
                description: declared.description,
                body: declared.body,
                tools: declared.tools,
                // 宿主声明没有 frontmatter，142 的 hidden 概念不适用（见字段文档）。
                hidden: false,
            };
            registry.insert(Arc::clone(&skill.id.0), skill);
        }
        SkillRegistry { skills: registry }
    }

    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// 装载进来的每个 skill 的 (id, 描述)，按 id 排序。宿主的 `/skills` 列表用它。
    pub fn listing(&self) -> Vec<(Arc<str>, Arc<str>)> {
        self.skills
            .values()
            .map(|s| (Arc::clone(&s.id.0), Arc::clone(&s.description)))
            .collect()
    }

    /// 按 id 精确查正文（137，`srv:skill/read` 用它）。**不滤 hidden**（142）：
    /// hidden 只挡索引，不挡读——已装载的 id 永远能读到正文。
    pub fn body_of(&self, id: &str) -> Option<Arc<str>> {
        self.skills.get(id).map(|skill| Arc::clone(&skill.body))
    }
}
