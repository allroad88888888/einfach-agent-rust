//! 011 验收「IO 失败时：append 不 panic、on_error 收到、内存侧调用方一切照旧」。
//!
//! 用一个**父目录不存在**的路径逼 `OpenOptions::open` 在 IO 线程启动时就失败——
//! 比「只读目录」更环境无关（部分沙箱/CI 以 root 跑测试时，权限位不生效，
//! 目录不存在这条路走到哪都一样失败）。

mod session_store_support;

use agent_store::SessionStore;
use agent_store::history::{Change, Entry};

use agent_runtime::Jsonl;
use session_store_support::{Val, collecting_on_error, temp_path};

type Backend = Jsonl<String, Val, u32>;

fn entry(seq: u64) -> Entry<String, Val, u32> {
    Entry { seq, meta: 1, changes: vec![Change { key: "a".to_string(), prev: Val(seq as i64), next: Val(seq as i64 + 1) }] }
}

#[test]
fn a_missing_parent_directory_reports_once_and_never_panics() {
    // `temp_path` 本身已经是「不预先创建」的文件路径；这里在它前面再插一层不存在的
    // 目录组件，让它连**父目录**都没有——`OpenOptions::new().create(true)` 不会帮忙
    // 建父目录，`open` 必然失败。
    let bad_path = temp_path("io-failure").with_file_name("nonexistent-parent").join("session.jsonl");
    let (errors, on_error) = collecting_on_error();
    let backend: Backend = Jsonl::new(&bad_path, on_error);

    // 构造本身不 panic、不阻塞——IO 线程起来之后才会发现打不开文件。给它一点时间
    // 处理完 open（下面第一次 append 之后的 flush 会顺带等到这一步完成）。
    backend.append(&entry(0));
    backend.set_cursor(1);
    backend.snapshot(&agent_store::Snapshot { values: vec![("a".to_string(), Val(1))] });
    backend.drop_oldest(1);
    backend.drop_after(0, 1);
    backend.flush(); // 排干——此时前面几条消息都已经被 IO 线程处理过（哪怕处理结果是「写不进」）

    let seen = errors.lock().unwrap().clone();
    assert!(!seen.is_empty(), "打不开文件应该经 on_error 报至少一次");
    assert!(
        seen.iter().all(|e| matches!(e, agent_runtime::SessionStoreError::Io { .. })),
        "这条路径上唯一可能的错误类别是 Io，见到别的说明分类分岔了：{seen:?}"
    );

    // load() 在文件从未真正建出来的情况下应该干脆地给 Absent，不是 panic、不是 Err。
    assert!(backend.load().is_absent());

    // 「内存侧调用方一切照旧」：这些方法调用本身不会让调用方自己的 History 出错——
    // SessionStore 只是被单向告知写入，从不回读、不会让上层的写入语句本身失败。
    // 这里额外证明「backend 挂了之后继续调用它」也不会连锁 panic——fire-and-forget
    // 的字面意思是「这次 IO 失败之后，端口还能继续正常被调用」。
    backend.append(&entry(1));
    backend.set_cursor(2);
    assert!(backend.load().is_absent());
}
