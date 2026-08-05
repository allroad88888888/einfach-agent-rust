//! 028 独立测试：`AgentId` 路径代数的边角情况。
//!
//! 只测路径运算本身（`root`/`child`/`parent`/`depth`/`is_ancestor_of`/
//! `is_descendant_of`），不涉及 `Session`。黑盒来源：`cargo doc -p agent-core
//! --no-deps` 的 `ids::AgentId` 页面 + docs/issues/028-multi-agent-graph.md
//! §1「AgentId 补路径语义」+ §「验收」第一条。不读 `src/ids/agent.rs` 源码。

use agent_core::AgentId;

#[test]
fn root_a1_is_not_an_ancestor_of_root_a10() {
    let root = AgentId::root();
    let a1 = root.child(1);
    let a10 = root.child(10);

    assert!(
        !a1.is_ancestor_of(&a10),
        "纯前缀匹配会把 a1 误判成 a10 的祖先"
    );
    assert!(!a10.is_ancestor_of(&a1));
    assert!(root.is_ancestor_of(&a1));
    assert!(root.is_ancestor_of(&a10));
}

/// 同一个坑在深一层的地方重现：`root/a1/a2` 不是 `root/a1/a20` 的祖先。
#[test]
fn the_boundary_bug_reappears_two_levels_down() {
    let base = AgentId::root().child(1);
    let a2 = base.child(2);
    let a20 = base.child(20);

    assert!(!a2.is_ancestor_of(&a20));
    assert!(!a20.is_ancestor_of(&a2));
    // 共同祖先关系不受影响。
    assert!(base.is_ancestor_of(&a2));
    assert!(base.is_ancestor_of(&a20));
}

/// 再深一层（三层深处的同型边界）：同样的坑不会因为多绕一层而消失或提前暴露。
#[test]
fn the_boundary_bug_reappears_three_levels_down() {
    let base = AgentId::root().child(1).child(1);
    let a3 = base.child(3);
    let a30 = base.child(30);

    assert!(!a3.is_ancestor_of(&a30));
    assert!(!a30.is_ancestor_of(&a3));
    assert!(base.is_ancestor_of(&a3));
    assert!(base.is_ancestor_of(&a30));
}

/// 严格性：自己不是自己的祖先，也不是自己的后代——否则跨 agent 读口会多出一个
/// 绕过 visibility 的自读后门（`AgentId` 模块文档原文的理由）。
#[test]
fn nobody_is_their_own_ancestor_or_descendant() {
    let root = AgentId::root();
    let deep = root.child(1).child(2).child(3);

    for id in [
        root.clone(),
        root.child(1),
        root.child(1).child(2),
        deep.clone(),
    ] {
        assert!(!id.is_ancestor_of(&id), "{id:?} 不该是自己的祖先");
        assert!(!id.is_descendant_of(&id), "{id:?} 不该是自己的后代");
    }
}

#[test]
fn parent_of_root_is_none_and_does_not_panic() {
    assert_eq!(AgentId::root().parent(), None);
}

/// `parent()` 一路走到 root 再往上：链条正确终止在 `None`，不会绕回 root 本身
/// 或者 panic。
#[test]
fn the_parent_chain_walks_back_to_root_and_stops_there() {
    let root = AgentId::root();
    let deep = root.child(1).child(2).child(3);

    let level2 = deep.parent().expect("depth 3 的 parent 该是 depth 2");
    assert_eq!(level2, root.child(1).child(2));

    let level1 = level2.parent().expect("depth 2 的 parent 该是 depth 1");
    assert_eq!(level1, root.child(1));

    let level0 = level1.parent().expect("depth 1 的 parent 该是 root");
    assert_eq!(level0, root);

    assert_eq!(level0.parent(), None, "root 再往上没有东西");
}

#[test]
fn depth_counts_separators_not_digits() {
    let root = AgentId::root();
    assert_eq!(root.depth(), 0);
    assert_eq!(root.child(1).depth(), 1);
    assert_eq!(root.child(1).child(2).depth(), 2);
    // 两位数的 seq 不该多算出一层深度——如果实现退化成按字符数或按位数判断，
    // 这条会翻车。
    assert_eq!(root.child(10).depth(), 1, "两位数字的 seq 不该多算一层深度");
    assert_eq!(root.child(1).child(20).depth(), 2);
}

/// `is_ancestor_of` 与 `is_descendant_of` 互为反演：对任意一对 id，
/// `x.is_ancestor_of(&y) == y.is_descendant_of(&x)`——覆盖祖先/后代、隔代、
/// 兄弟、反方向、自身六种关系形状。
#[test]
fn is_ancestor_of_and_is_descendant_of_are_exact_inverses() {
    let root = AgentId::root();
    let a1 = root.child(1);
    let a2 = root.child(2);
    let a1_b1 = a1.child(1);

    let pairs = [
        (root.clone(), a1.clone()),    // 祖先/后代（一层）
        (a1.clone(), a1_b1.clone()),   // 祖先/后代（再一层）
        (root.clone(), a1_b1.clone()), // 隔代祖先
        (a1.clone(), a2.clone()),      // 兄弟：两个方向都不成立
        (a1.clone(), root.clone()),    // 方向反过来：都不成立
        (a1.clone(), a1.clone()),      // 自身：都不成立
    ];

    for (x, y) in pairs {
        assert_eq!(
            x.is_ancestor_of(&y),
            y.is_descendant_of(&x),
            "is_ancestor_of/is_descendant_of 对 ({x:?}, {y:?}) 没有互逆"
        );
        assert_eq!(
            x.is_descendant_of(&y),
            y.is_ancestor_of(&x),
            "反过来调用同一对参数也该互逆"
        );
    }
}
