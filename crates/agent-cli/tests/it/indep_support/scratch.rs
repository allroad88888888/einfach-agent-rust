//! 每个测试独占的 scratch 目录：假 `providers.toml`、会话 jsonl 文件都落在这里，
//! 进程退出（`Drop`）时尽力清理，不留垃圾在系统临时目录里。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct Scratch {
    pub dir: PathBuf,
}

impl Scratch {
    /// `tag` 只是用来让目录名可读（比如测试名的缩写），不参与唯一性保证——
    /// 唯一性靠进程 id + 单调计数器 + 纳秒时间戳。
    pub fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("agent-cli-indep-{tag}-{}-{n}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        Scratch { dir }
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// 写一份指向假服务器的 `providers.toml`，只有 deepseek 一家，key 随便填。
    pub fn write_providers_toml(&self, base_url: &str) -> PathBuf {
        let content = format!(
            "[providers.deepseek]\n\
             api_key = \"fake-key-0123456789\"\n\
             base_url = \"{base_url}\"\n\
             model = \"deepseek-v4-pro\"\n\
             \n\
             [default]\n\
             provider = \"deepseek\"\n"
        );
        let path = self.path("providers.toml");
        std::fs::write(&path, content).expect("write providers.toml");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
