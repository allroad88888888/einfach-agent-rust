//! 模型**写在参数里**的一个工具名 → 规范工具全名（issue 050）。
//!
//! # 为什么需要这一步
//!
//! 工具名同时是两种东西：
//!
//! - **函数名**：wire 上的 `tools[].function.name`，OpenAI 惯例只许
//!   `[A-Za-z0-9_-]`，所以 adapter 把 `srv:fs/list` 转义成 `srv_3Afs_2Flist`
//!   （`agent_providers::wire_name`）。模型调工具时写回来的也是这个名字，
//!   adapter 在累积器里解回规范名——**这条路 encode↔decode 对称，没有问题**。
//! - **参数值**：`srv:agent/spawn` 的 `tools` 子集，以及以后任何「吃一个工具名」
//!   的参数。模型只能照抄它看见的名字，写进来的就是 wire 名；而 adapter 不知道
//!   哪个 arg 是工具名，**不会也不该**去解码任意 JSON 参数。
//!
//! 于是 wire 名直达宿主的校验（`spawn_tool::check_subset`），跟规范名对不上、
//! 被拒。049 的真机 dogfood 逮到过这一幕：模型传 `srv_3Afs_2Flist`。
//!
//! 更糟的是模型同时看得见**两种拼法**——工具描述正文是原样透传的，
//! `srv:agent/collect` 的描述里就写着「先用 srv:agent/status 看谁已经 Done」，
//! 而函数列表里那一栏是 `srv_3Aagent_2Fstatus`。既然两种拼法都是我们自己喂给它的，
//! **两种都该认**。
//!
//! # 为什么解码住在宿主，不住 core、也不住 adapter
//!
//! - core（红线 12）：它连 `wire_name` 的类型都够不着，也不该懂任何 wire 形状。
//! - adapter：它不知道哪个 arg 是工具名。要它知道，就得让每个工具的 schema 标注
//!   「这个字段是工具名」再让 adapter 遍历入参——那是把宿主的工具语义推进一个
//!   纯函数层，接缝画错的典型。
//! - **宿主**：它同时持有权威工具表和 adapter，两边都够得着。按名字分流本来就是
//!   宿主的活（`dispatch` 就是这么干的），这里没有任何模型相关判断。
//!
//! # 规则：先精确、后解码，且**只映到已经存在的名字上**
//!
//! [`resolve`] 永远在 `known` 里挑一个出来，或者什么都不挑——它**造不出**新名字。
//! 所以「模型瞎编一个工具名」照旧被拒，050 修的只是「名字对、拼法是我们自己
//! 编码出来的那种」这一格。
//!
//! 精确匹配优先于解码，是因为 `from_wire` 对少数规范名**不是恒等的**：一个真
//! 叫 `a_3Ab` 的工具会被解成 `a:b`。这种名字 `to_wire` 会退到严格档
//! （`a_5F3Ab`），两边都还原得回去；精确优先则保证它永远先被自己接住，
//! 不会被另一个工具影子里的解码结果抢走。
//!
//! 反过来，`known` 里两个不同的规范名不可能有同一个 wire 名——`to_wire` 的自校验
//! 保证了单射（`wire_name` 的模块文档）——所以「解码后再匹配」不存在二义。

use std::sync::Arc;

use agent_providers::wire_name;

/// 把模型写在参数里的一个工具名解析成 `known` 里的规范全名。
///
/// `None` = `known` 里没有这个工具（两种拼法都不是）——调用方照旧拒绝，
/// 并把「你有哪些」告诉模型（决策 20 的自纠兜底）。
///
/// `known` 是调用方手上那份**权威**清单：spawn 传的是调用者自己的
/// `subagent::allowed_names`（子拿不到父没有的工具，那是提权），
/// 别的调用点传自己那份。这个函数不认识「工具表」这个概念，只认识一个字符串切片。
pub(crate) fn resolve<'a>(given: &str, known: &'a [Arc<str>]) -> Option<&'a Arc<str>> {
    if let Some(hit) = known.iter().find(|name| &***name == given) {
        return Some(hit);
    }
    let decoded = wire_name::from_wire(given);
    known.iter().find(|name| ***name == *decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known() -> Vec<Arc<str>> {
        ["srv:fs/list", "srv:agent/spawn", "mcp:everything/echo", "read_file"]
            .iter()
            .map(|n| Arc::from(*n))
            .collect()
    }

    /// 规范名原样通过——模型写对了的那条路一个字节都没变。
    #[test]
    fn a_canonical_name_resolves_to_itself() {
        for name in ["srv:fs/list", "mcp:everything/echo", "read_file"] {
            assert_eq!(&**resolve(name, &known()).unwrap(), name);
        }
    }

    /// 050 的现象：模型照抄它在函数列表里看到的名字。`mcp:` 前缀走同一条路
    /// （M6 的工具也是被同一份 `to_wire` 转义的）。
    #[test]
    fn a_wire_name_resolves_back_to_the_canonical_one() {
        assert_eq!(&**resolve("srv_3Afs_2Flist", &known()).unwrap(), "srv:fs/list");
        assert_eq!(&**resolve("srv_3Aagent_2Fspawn", &known()).unwrap(), "srv:agent/spawn");
        assert_eq!(
            &**resolve("mcp_3Aeverything_2Fecho", &known()).unwrap(),
            "mcp:everything/echo"
        );
    }

    /// 转义过的每一个名字都必须解得回来——用 `to_wire` 现算，
    /// 而不是把 `_3A` 抄进断言里：编码规则哪天改了，坏的该是编解码那对函数
    /// 自己的测试，不是这里。
    #[test]
    fn every_known_name_round_trips_through_its_wire_form() {
        for name in known() {
            let wire = wire_name::to_wire(&name);
            assert_eq!(resolve(&wire, &known()).unwrap(), &name, "{name} 的 wire 名解不回来");
        }
    }

    /// **造不出新名字**：不在清单里的照旧是 `None`，两种拼法都是。
    #[test]
    fn an_unknown_name_stays_unknown() {
        assert!(resolve("srv:shell/exec", &known()).is_none());
        assert!(resolve("srv_3Ashell_2Fexec", &known()).is_none());
        assert!(resolve("", &known()).is_none());
    }

    /// 精确匹配优先：一个真叫 `a_3Ab` 的工具被自己接住，不会被 `a:b` 抢走。
    #[test]
    fn an_exact_match_wins_over_decoding() {
        let both: Vec<Arc<str>> = ["a:b", "a_3Ab"].iter().map(|n| Arc::from(*n)).collect();
        assert_eq!(&**resolve("a_3Ab", &both).unwrap(), "a_3Ab");
        assert_eq!(&**resolve("a:b", &both).unwrap(), "a:b");
        // 严格档的 wire 名只可能是 `a_3Ab` 的：`a:b` 的可读档是 `a_3Ab`，
        // 于是 `a_3Ab` 自校验失败退到 `a_5F3Ab`，两者不撞。
        assert_eq!(&**resolve("a_5F3Ab", &both).unwrap(), "a_3Ab");
    }
}
