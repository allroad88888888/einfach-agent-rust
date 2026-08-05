//! 黑盒驱动真二进制：`env!("CARGO_BIN_EXE_agent-cli")` spawn 子进程，stdin
//! 管道喂行，stdout/stderr 各开一个读线程采集进共享 buffer 供轮询断言。
//!
//! 实测确认（真跑二进制、非读源码取证）：启动横幅、`会话文件=`、连接错误、
//! 会话文件损坏警告都走 **stderr**；对话内容、提示符、undo/redo、工具调用
//! 日志、`[会话已恢复]` 都走 **stdout**。`combined_output()` 把两路拼在一起，
//! 大多数断言不关心具体是哪一路；需要精确定位的断言用 `stdout_snapshot()` /
//! `stderr_snapshot()` 单独取。

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct CliProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
}

impl CliProcess {
    /// `AGENT_PROVIDERS_CONFIG` 指到假 providers.toml；`session_path` 给了就
    /// 拼 `--session <path>`（027 实做记录：`--session` 参数或
    /// `AGENT_SESSION_PATH`，这里固定用命令行参数那条路）。
    pub fn spawn(providers_config: &Path, session_path: Option<&Path>) -> Self {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_agent-cli"));
        cmd.env("AGENT_PROVIDERS_CONFIG", providers_config);
        if let Some(session) = session_path {
            cmd.arg("--session").arg(session);
        }
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = cmd.spawn().expect("spawn agent-cli binary");
        let stdin = child.stdin.take().expect("cli stdin");
        let stdout_pipe = child.stdout.take().expect("cli stdout");
        let stderr_pipe = child.stderr.take().expect("cli stderr");

        let stdout = Arc::new(Mutex::new(Vec::new()));
        let stderr = Arc::new(Mutex::new(Vec::new()));
        spawn_byte_reader(stdout_pipe, Arc::clone(&stdout));
        spawn_byte_reader(stderr_pipe, Arc::clone(&stderr));

        CliProcess { child, stdin, stdout, stderr }
    }

    /// 写一行到子进程 stdin（自动补 `\n`），立刻 flush。
    pub fn send_line(&mut self, line: &str) {
        writeln!(self.stdin, "{line}").expect("write to cli stdin");
        self.stdin.flush().expect("flush cli stdin");
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// 对子进程发信号（`sig` 形如 `"-INT"`、`"-9"`），走系统 `kill` 命令而不是
    /// 引入 libc 依赖——效果跟真实 Ctrl-C / `kill -9` 完全一样。
    pub fn send_signal(&self, sig: &str) {
        Command::new("kill").arg(sig).arg(self.pid().to_string()).status().expect("send signal to cli process");
    }

    pub fn stdout_snapshot(&self) -> String {
        String::from_utf8_lossy(&self.stdout.lock().unwrap()).into_owned()
    }

    pub fn stderr_snapshot(&self) -> String {
        String::from_utf8_lossy(&self.stderr.lock().unwrap()).into_owned()
    }

    pub fn combined_output(&self) -> String {
        let mut out = self.stdout_snapshot();
        out.push_str(&self.stderr_snapshot());
        out
    }

    /// 轮询直到合并输出里出现 `needle`，超时返回 `false`（调用方自己决定
    /// 失败时把 `combined_output()` 打进断言消息，方便看真实发生了什么）。
    pub fn wait_for(&self, needle: &str, timeout: Duration) -> bool {
        let start = Instant::now();
        loop {
            if self.combined_output().contains(needle) {
                return true;
            }
            if start.elapsed() > timeout {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// 轮询直到子进程退出，返回退出状态；超时返回 `None`（进程仍然活着）。
    pub fn wait_exit(&mut self, timeout: Duration) -> Option<ExitStatus> {
        let start = Instant::now();
        loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                return Some(status);
            }
            if start.elapsed() > timeout {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for CliProcess {
    fn drop(&mut self) {
        // 测试失败提前返回时子进程可能还活着——不放任它变成孤儿。
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// 按原始字节读，不按行读。
///
/// 实测发现（真跑二进制取证，不是猜的）：REPL 的提示符 `"> "` 打印之后
/// **没有换行**就 flush 等待输入了——`BufRead::read_line` 的语义是“找到
/// `\n` 或者遇到 EOF 才返回”，那样的话提示符会一直卡在读线程内部的缓冲区
/// 里，`wait_for` 永远看不到它，直到下一行真正换行的内容出现为止（对
/// 大多数断言无所谓，但取消测试需要在流式内容中途、真正换行之前就发现
/// 部分内容并动手按 Ctrl-C，行缓冲在那种场景下会直接把测试卡到超时）。
/// 按原始字节收，snapshot 时才用 `from_utf8_lossy` 解码，避免这个坑。
fn spawn_byte_reader<R: Read + Send + 'static>(mut pipe: R, buf: Arc<Mutex<Vec<u8>>>) {
    std::thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) | Err(_) => return,
                Ok(n) => buf.lock().unwrap().extend_from_slice(&chunk[..n]),
            }
        }
    });
}
