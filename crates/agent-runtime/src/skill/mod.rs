//! skill 装载（039，决策 21）：宿主侧的 `SkillRegistry`——从磁盘 `SKILL.md` 把
//! skill 读进来，展开成「常驻索引」（进 system 前缀）和「激活时的注入」
//! （正文进 `late_system`、携带的工具进 `late_tools`）。
//!
//! # 边界：registry 是宿主的，不是 core 的
//!
//! `agent-core` 只认识「哪些 skill id 被激活」（`Slot::SkillsActive`，红线 12 的
//! 精神：core 不认识「skill 内容」这个概念）。**内容活在这里**——store 之外、可以
//! 做 IO（红线 7 只约束 core/store）。恢复一个会话时激活集这个 primitive 自动回来、
//! 正文从这个 registry **现取**：registry 内容在两次运行之间漂移了（改了正文、删了
//! 一个 skill），激活集里那个 id 要么取到新正文、要么取不到（[`injection`] 当它
//! 没激活）。这个漂移是刻意的取舍，理由见 `agent-core` 的 `command/skill.rs`。
//!
//! # 三个子模块
//!
//! - [`yaml`]：SKILL.md frontmatter 的缩进式 YAML 子集解析（无外部依赖）。
//! - [`load`]：目录遍历 + frontmatter/正文切分 + 建 [`Skill`]。
//! - [`tool`]：`srv:skill/activate` / `srv:skill/deactivate` 的声明、入参解析、
//!   以及 dispatch 截获点。

mod load;
mod tool;
mod yaml;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use agent_core::{SkillId, SystemChunk, ToolSpec};

pub use load::SkillLoadError;
pub use tool::{SKILL_ACTIVATE, SKILL_DEACTIVATE, activate_spec, deactivate_spec};
pub(crate) use tool::intercept;

/// 常驻索引那段 system 的标签（进日志，不进 prompt——见 `SystemChunk`）。
const INDEX_LABEL: &str = "skill-index";

/// 一个装载进来的 skill：id（= frontmatter 的 `name`）、进索引的那行描述、激活时
/// 注入的正文、以及可选携带的工具。
struct Skill {
    id: SkillId,
    description: Arc<str>,
    body: Arc<str>,
    tools: Vec<ToolSpec>,
}

/// 宿主持有的 skill 目录索引。**用 `BTreeMap` 不是 `HashMap`（红线 11）**：索引和
/// 注入内容都会进 prompt，迭代顺序必须逐字节确定。
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
        SkillRegistry { skills: BTreeMap::new() }
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
    /// 加一条合并路径，那时才轮得到 064 §6 那条闸（宿主声明的 id 撞上磁盘已装载的
    /// id → 400，跟 061 同一处，那一刻客户端还在线）。**绝不能**让 `BTreeMap::insert`
    /// 顺手把它变成静默的后来居上——那是本条最坏的结局。
    ///
    /// id 在一份声明内部的唯一性由 061 的校验保证（`DuplicateSkill` → 400），
    /// 所以这里不会真的撞键。
    pub fn from_host_skills(skills: Vec<agent_core::HostSkill>) -> Self {
        let skills = skills
            .into_iter()
            .map(|declared| {
                let skill = Skill {
                    id: declared.id,
                    description: declared.description,
                    body: declared.body,
                    tools: declared.tools,
                };
                (Arc::clone(&skill.id.0), skill)
            })
            .collect();
        SkillRegistry { skills }
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

    /// 常驻索引：每个 skill **一行**「id: 描述」，按 id 排序（红线 11）。它跟工具表
    /// 一样是**随时都在**的稳定前缀的一部分（宿主把它放进 `Ingredients::system`），
    /// 不是激活时才注入的——所以模型第一轮、激活之前就能发现有哪些 skill。
    ///
    /// 空 registry → **空文本**：`messages::system_text` 会把空段滤掉，于是「没装
    /// 任何 skill 的会话」的前缀跟 039 之前逐字节一致（向后兼容）。
    pub fn skill_index_chunk(&self) -> SystemChunk {
        let text = if self.skills.is_empty() {
            String::new()
        } else {
            let mut out = String::from(
                "可用的 skill（用 srv:skill/activate 激活、srv:skill/deactivate 停用）：",
            );
            for skill in self.skills.values() {
                out.push('\n');
                out.push_str(&skill.id.0);
                out.push_str(": ");
                out.push_str(&skill.description);
            }
            out
        };
        SystemChunk { label: Arc::from(INDEX_LABEL), text: Arc::from(text) }
    }

    /// 这个 skill 装载进来了吗（dispatch 截获激活时先查这个，没有就回 is_error）。
    pub(crate) fn contains(&self, id: &str) -> bool {
        self.skills.contains_key(id)
    }

    /// 「你有哪些 skill」的一行（激活一个不存在的 id 时回给模型，让它自己收敛）。
    pub(crate) fn known_ids(&self) -> Vec<&str> {
        self.skills.keys().map(|k| &**k).collect()
    }

    /// 把一组激活的 skill id 展开成本轮的注入料：正文进 `late_system`（一个 skill
    /// 一个 `SystemChunk`），携带的工具进 `late_tools`。顺序跟着 `active` 走
    /// （`active_skills` 已经是排序的，红线 11）；registry 里查不到的 id **静默跳过**
    /// ——那是「激活集里有、registry 却没有」的漂移（删了个 skill 又恢复了老会话），
    /// 当它没激活是最不惊扰的选择。
    pub(crate) fn injection(&self, active: &[SkillId]) -> (Vec<SystemChunk>, Vec<ToolSpec>) {
        let mut late_system = Vec::new();
        let mut late_tools = Vec::new();
        for id in active {
            let Some(skill) = self.skills.get(&id.0) else {
                continue;
            };
            late_system.push(SystemChunk {
                label: Arc::from(format!("skill:{}", skill.id.0)),
                text: Arc::clone(&skill.body),
            });
            late_tools.extend(skill.tools.iter().cloned());
        }
        (late_system, late_tools)
    }
}
