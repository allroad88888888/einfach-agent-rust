//! 校验过的声明 → 这一个会话的工具表要的料（062，接缝见
//! `docs/HOST-CAPABILITIES.md` §五）。
//!
//! 061 校验完就把 `capabilities` 丢掉了；这里接上它：把 [`CapabilityTool`] 翻成
//! `agent_core::ToolSpec` + 一个 `Reversibility`，交给
//! [`SessionTemplate::open_spec`](crate::http::config::SessionTemplate::open_spec)
//! 塞进**这一次**的 `OpenSpec`。装配的另一半（追加表尾、按名字排序、可逆性另挂一张
//! 表）在 `agent_runtime::ToolTable::with_host_tools`——这里只做翻译，不做排序也不
//! 决定位置，那两件事是红线 11 的落点，该在最靠近 prompt 字节的那一层白拿。
//!
//! # 「没说」落保守 `Irreversible`（§五）
//!
//! 宿主说了 `pure` 就按 `pure` 办——**这是它自己的数据，它自己负责**（跟 MCP 那次
//! 决策 22 同一条规矩，但理由不同：MCP 的 `readOnlyHint` 来自第三方 server，宿主是
//! 企业自己的代码）。没说则落 `Irreversible`：「没说」不能推定为「安全」，`/undo`
//! 撞到它会停下来问。这个解释**刻意不在协议层做**（061 的 `reversibility` 缺省解析
//! 成 `None`，如实记录说没说），装配这一层才是它的位置。
//!
//! # 这一层的入参是「一份声明」，不是「一次请求」
//!
//! [`host_tools`] 吃的是已经解析好的 [`Capabilities`]，吐的是纯 `agent_core` 数据
//! （`Vec<(ToolSpec, Reversibility)>`）——两头都不认识 HTTP，也不认识
//! `CreateSessionRequest`。这不是洁癖：073 会把声明挪进 store（建会话时 journaled、
//! 恢复时从日志回放，宿主**不必**在重连时重报一遍——历史对话就该在它当初那份工具表
//! 下原样复刻），那时「声明从哪来」会从请求换成回放，而这一层与它下游的装配一行都
//! 不用改。
//!
//! # 两个出口，形状对称
//!
//! [`host_tools`] 吐 `Vec<(ToolSpec, Reversibility)>`（进工具表），[`host_skills`]
//! 吐 `Vec<HostSkill>`（进这个会话的 `SkillRegistry`）。两者都只翻译、不排序、不决
//! 定位置——排序是红线 11 的落点，该在最靠近 prompt 字节的那一层白拿
//! （`ToolTable::with_host_tools` 排工具，`SkillRegistry` 的 `BTreeMap` 排 skill）。
//!
//! **skill 自带的工具不进工具表**：决策 27（M15）之后 v1 根本不支持它——校验层
//! （`validate.rs`）已经把 `capabilities.skills[].tools` 非空整份 400，本函数这里
//! 走到的 `skill.tools` 因此在实践中恒空。翻译逻辑留着只是为了 [`HostSkill`] 的
//! 字段形状完整（老 journal 反序列化兼容），不代表这条路今天还有产出。
//!
//! # skill 自带工具的 `reversibility` 旁挂保存
//!
//! 这一步把（恒空的）执行元数据按名字写进 [`HostSkill::tool_reversibility`]。
//! 它不进入 `ToolSpec`，因而不改变 skill index 或正文的 prompt 字节。

use std::sync::Arc;

use agent_core::{HostSkill, Reversibility, SkillId, ToolSpec};

use super::{Capabilities, CapabilityTool};

/// 把 `capabilities.tools` 翻成 `ToolTable` 要的 `(ToolSpec, Reversibility)`。
///
/// 没带 `capabilities`（老调用方）或者声明为空 → 空 `Vec`，下游一路空操作，工具表
/// 与 062 之前逐字节相同。**只翻译顶层 `tools`**，理由见模块文档。
pub(in crate::http) fn host_tools(
    capabilities: Option<&Capabilities>,
) -> Vec<(ToolSpec, Reversibility)> {
    let Some(capabilities) = capabilities else {
        return Vec::new();
    };
    capabilities
        .tools
        .iter()
        .map(|tool| {
            let spec = tool_spec(tool);
            (spec, reversibility(tool))
        })
        .collect()
}

/// 把 `capabilities.skills` 翻成 `SkillRegistry` 要的 [`HostSkill`]（064）。
///
/// 没带 `capabilities` 或者一个 skill 都没声明 → 空 `Vec`，下游 registry 为空、
/// 工具表不接 `.with_skills(..)`、常驻索引是空文本，这个会话跟 064 之前逐字节相同。
///
/// `id`/`description`/`body` 直接进（前两个进常驻索引，第三个按需经
/// `srv:skill/read` 取，139/141）；自带工具的三个 prompt 字段仍原样搬（今天恒空，
/// 见模块文档），可逆性按名字旁挂保存。
pub(in crate::http) fn host_skills(capabilities: Option<&Capabilities>) -> Vec<HostSkill> {
    let Some(capabilities) = capabilities else {
        return Vec::new();
    };
    capabilities
        .skills
        .iter()
        .map(|skill| {
            let mut tools: Vec<ToolSpec> = skill.tools.iter().map(tool_spec).collect();
            tools.sort_by(|a, b| a.name.cmp(&b.name));
            let tool_reversibility = skill
                .tools
                .iter()
                .map(|tool| (Arc::from(tool.name.as_str()), reversibility(tool)))
                .collect();
            HostSkill {
                id: SkillId::new(skill.id.as_str()),
                description: Arc::from(skill.description.as_str()),
                body: Arc::from(skill.body.as_str()),
                tools,
                tool_reversibility,
            }
        })
        .collect()
}

/// 把 `capabilities.prefix` 翻成 `ToolTable::with_host_prefix` 要的 `(name, text)`
/// 对（156，M17）。同 [`host_tools`]/[`host_skills`]：只翻译、不排序、不决定
/// 位置（红线 11 那两下在更靠近落盘/prompt 的地方各做一次）。没带 `capabilities`
/// 或声明为空 → 空 `Vec`，跟 156 之前逐字节相同。
pub(in crate::http) fn host_prefix(capabilities: Option<&Capabilities>) -> Vec<(Arc<str>, Arc<str>)> {
    let Some(capabilities) = capabilities else {
        return Vec::new();
    };
    capabilities
        .prefix
        .iter()
        .map(|p| (Arc::from(p.name.as_str()), Arc::from(p.text.as_str())))
        .collect()
}

/// 宿主省略可逆性时落保守值；顶层工具与 skill 工具共用同一条解释。
fn reversibility(tool: &CapabilityTool) -> Reversibility {
    tool.reversibility
        .map_or(Reversibility::Irreversible, Reversibility::from)
}

/// 一条工具声明 → `ToolSpec`：三个**进 prompt** 的字段原样搬。顶层的和 skill 自带
/// 的过同一处——它们在 061 就是同一个类型、同一条校验，翻译当然也该只有一处
/// （两处一旦漂开，症状是「同一份 schema 在顶层和 skill 里渲染出不同字节」）。
fn tool_spec(tool: &CapabilityTool) -> ToolSpec {
    ToolSpec {
        name: Arc::from(tool.name.as_str()),
        description: Arc::from(tool.description.as_str()),
        // `schema` 原样进 prompt——不重排、不改写（061 已经把它当
        // `serde_json::Value` 原样收下，对象后端是 `BTreeMap`，逐字节确定）。
        schema: Arc::new(tool.schema.clone()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn caps(value: serde_json::Value) -> Capabilities {
        serde_json::from_value(value).expect("该解析成功")
    }

    /// 三个字段原样落进 `ToolSpec`（它们进 prompt，改写一个字都是改 prompt 字节）。
    #[test]
    fn the_three_prompt_facing_fields_are_carried_over_as_is() {
        let schema = json!({ "type": "object", "properties": { "id": { "type": "string" } } });
        let declared = caps(json!({
            "tools": [ { "name": "web:crm/lookup", "description": "按客户 ID 查 CRM 档案", "schema": schema } ]
        }));

        let tools = host_tools(Some(&declared));
        assert_eq!(tools.len(), 1);
        assert_eq!(&*tools[0].0.name, "web:crm/lookup");
        assert_eq!(&*tools[0].0.description, "按客户 ID 查 CRM 档案");
        assert_eq!(*tools[0].0.schema, schema);
    }

    /// 062 验收：声明了就用；**没声明落保守 `Irreversible`**（§五）。
    #[test]
    fn a_declared_reversibility_is_used_and_a_missing_one_falls_conservative() {
        let declared = caps(json!({
            "tools": [
                { "name": "web:crm/lookup", "reversibility": "pure" },
                { "name": "web:crm/draft", "reversibility": "reversible" },
                { "name": "web:crm/close", "reversibility": "irreversible" },
                { "name": "desk:clipboard/write" }
            ]
        }));

        let levels: Vec<Reversibility> = host_tools(Some(&declared))
            .into_iter()
            .map(|(_, r)| r)
            .collect();
        assert_eq!(
            levels,
            vec![
                Reversibility::Pure,
                Reversibility::Reversible,
                Reversibility::Irreversible,
                Reversibility::Irreversible,
            ],
            "最后那个没声明的必须是保守值——「没说」不能推定为「安全」"
        );
    }

    /// 不带 `capabilities` 的老调用方、以及空声明：空 `Vec`（下游空操作，向后兼容
    /// 那条验收的起点）。
    #[test]
    fn no_declaration_means_nothing_to_inject() {
        assert!(host_tools(None).is_empty());
        assert!(host_tools(Some(&caps(json!({})))).is_empty());
        assert!(host_tools(Some(&caps(json!({ "tools": [] })))).is_empty());
    }

    /// skill 自带的工具**不进工具表**——它们跟着 [`host_skills`] 走（今天 v1 不
    /// 支持非空的 skill tools，`validate.rs` 会在更早的校验层 400，这条只钉
    /// `assemble.rs` 自身的翻译边界：即便喂给它，也不会串进顶层工具表）。
    #[test]
    fn tools_carried_by_a_skill_do_not_enter_the_tool_table() {
        let declared = caps(json!({
            "tools": [ { "name": "web:crm/lookup" } ],
            "skills": [ { "id": "crm-flow", "tools": [ { "name": "web:crm/close-ticket" } ] } ]
        }));
        let names: Vec<String> = host_tools(Some(&declared))
            .iter()
            .map(|(s, _)| s.name.to_string())
            .collect();
        assert_eq!(names, vec!["web:crm/lookup"]);
    }

    /// 064：四个字段原样搬进 [`HostSkill`]——`description` 会变成常驻索引那一行、
    /// `body` 按需经 `srv:skill/read` 取（139/141）、自带的工具字段原样搬但今天
    /// 没有任何路径会执行或注入它们（见模块文档）。
    #[test]
    fn a_declared_skill_carries_its_index_line_body_and_tools() {
        let schema = json!({ "type": "object", "properties": { "ticket": { "type": "string" } } });
        let declared = caps(json!({
            "skills": [ {
                "id": "crm-flow",
                "description": "处理客户工单的标准流程",
                "body": "第一步……第二步……",
                "tools": [ { "name": "web:crm/close-ticket", "description": "关单", "schema": schema } ]
            } ]
        }));

        let skills = host_skills(Some(&declared));
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].id.as_str(), "crm-flow");
        assert_eq!(&*skills[0].description, "处理客户工单的标准流程");
        assert_eq!(&*skills[0].body, "第一步……第二步……");
        assert_eq!(&*skills[0].tools[0].name, "web:crm/close-ticket");
        assert_eq!(*skills[0].tools[0].schema, schema);
        assert_eq!(
            skills[0].tool_reversibility["web:crm/close-ticket"],
            Reversibility::Irreversible
        );
    }

    /// skill 工具显式值原样保存；省略值跟顶层工具一样保守落 `Irreversible`。
    #[test]
    fn skill_tool_reversibility_is_preserved_and_missing_is_conservative() {
        let declared = caps(json!({
            "skills": [ {
                "id": "source-code",
                "tools": [
                    { "name": "web:source/search", "reversibility": "pure" },
                    { "name": "web:source/cache", "reversibility": "reversible" },
                    { "name": "web:source/pull", "reversibility": "irreversible" },
                    { "name": "web:source/read" }
                ]
            } ]
        }));

        let skills = host_skills(Some(&declared));
        let levels = &skills[0].tool_reversibility;
        let names: Vec<&str> = skills[0].tools.iter().map(|tool| &*tool.name).collect();
        assert_eq!(
            names,
            vec![
                "web:source/cache",
                "web:source/pull",
                "web:source/read",
                "web:source/search"
            ]
        );
        assert_eq!(levels["web:source/search"], Reversibility::Pure);
        assert_eq!(levels["web:source/cache"], Reversibility::Reversible);
        assert_eq!(levels["web:source/pull"], Reversibility::Irreversible);
        assert_eq!(levels["web:source/read"], Reversibility::Irreversible);
    }

    /// 没带 `capabilities`、空声明、只声明了工具——三种情况下 skill 那一侧都是空，
    /// registry 因此为空、工具表不接 `.with_skills(..)`（向后兼容那条验收的起点）。
    #[test]
    fn no_skill_declaration_means_an_empty_registry() {
        assert!(host_skills(None).is_empty());
        assert!(host_skills(Some(&caps(json!({})))).is_empty());
        assert!(host_skills(Some(&caps(json!({ "skills": [] })))).is_empty());
        assert!(host_skills(Some(&caps(json!({ "tools": [ { "name": "web:x/y" } ] })))).is_empty());
    }

    /// 缺 `description`/`body` 的 skill 是**合法**的（061 的形状层给了默认空串），
    /// 翻译出来就是空字符串——不是 `None`、也不是被跳过。空描述的索引行仍然占一行，
    /// 那是宿主自己的选择，server 不替它编一句话。
    #[test]
    fn a_sparse_skill_declaration_translates_to_empty_strings_not_a_guess() {
        let skills = host_skills(Some(&caps(json!({ "skills": [ { "id": "bare" } ] }))));
        assert_eq!(skills.len(), 1);
        assert_eq!(&*skills[0].description, "");
        assert_eq!(&*skills[0].body, "");
        assert!(skills[0].tools.is_empty());
    }
}
