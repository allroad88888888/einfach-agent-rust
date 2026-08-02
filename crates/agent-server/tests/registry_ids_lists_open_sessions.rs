//! `SessionRegistry::ids()`（035）：宿主优雅关闭时用它决定该给谁发 `close`
//! （`crate::http::SessionsHandle::close_all` 内部就是「`ids()` 再挨个
//! `close()`」，这条测试钉住 registry 这一层的语义，`SessionsHandle` 自己的
//! 测试只需要证明它薄薄转发了这两个调用）。

mod support;

#[test]
fn ids_reflects_opens_and_closes() {
    let registry = agent_server::SessionRegistry::new();
    assert!(registry.ids().is_empty(), "全新表该是空的");

    registry.open(support::open_spec("a", "http://127.0.0.1:1/unused".to_string(), None)).unwrap();
    registry.open(support::open_spec("b", "http://127.0.0.1:1/unused".to_string(), None)).unwrap();

    let mut ids: Vec<String> = registry.ids().iter().map(|id| id.to_string()).collect();
    ids.sort();
    assert_eq!(ids, vec!["a".to_string(), "b".to_string()]);

    registry.close(&agent_server::SessionId::from("a")).unwrap();
    let ids: Vec<String> = registry.ids().iter().map(|id| id.to_string()).collect();
    assert_eq!(ids, vec!["b".to_string()], "close 之后该从表里摘掉");
}

/// `close_all` 的核心手法（`ids()` 再挨个 `close()`）本身用真实 registry 走一遍：
/// 多个 session 同时挂着，逐个关掉之后表清空——`SessionsHandle::close_all`
/// （`crate::http` 模块）就是这段逻辑套一层 `AppState` 的薄壳。
#[test]
fn closing_every_id_empties_the_table() {
    let registry = agent_server::SessionRegistry::new();
    for name in ["x", "y", "z"] {
        registry.open(support::open_spec(name, "http://127.0.0.1:1/unused".to_string(), None)).unwrap();
    }
    for id in registry.ids() {
        registry.close(&id).unwrap();
    }
    assert!(registry.ids().is_empty(), "全部 close 完，表该是空的");
}
