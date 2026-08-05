//! stdio 传输：spawn 子进程，newline-delimited JSON-RPC 读写。**只管字节怎么进出
//! 子进程**——一行是不是合法 JSON-RPC、`id` 对不对，都是上层 `client` 的事；这里
//! 只知道「一行」和「进程死没死」。
//!
//! 照抄 `agent-tools/src/shell.rs` 的子进程模式：后台线程 + `mpsc` +
//! `recv_timeout` 换成「阻塞读但能被外部超时打断」（std 没有自带「读一行但最多
//! 等 N 秒」的 API）。不同点：shell 是一次性跑完等 `wait_with_output`；这里的
//! 子进程是**长驻 server**，要反复读多行，所以读线程常驻，每读到一行就通过
//! channel 转发一次，直到 EOF 才收工。
//!
//! **全局规则「后台跑 CLI 必须显式关 stdin」的镜像**：我们是 spawn 别人的 CLI，
//! 要控制它的 stdin——每条请求写完立刻 flush，不攒批，子进程不会因为「以为还有
//! 输入没写完」而卡住。子进程本身的收尾（关 stdin、杀、收尸）在 [`Drop`]。

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// stdout 尾巴只留最近这么多行，诊断用，不需要全量。
const STDERR_TAIL_LINES: usize = 20;

/// stdio 传输层的失败。**未知不猜成成功**——EOF、超时、IO 错误各自一个变体，
/// 上层 `client` 靠它们决定要不要把 stderr 尾巴打进日志、要不要重连。
#[derive(Debug)]
pub enum TransportError {
    /// `Command::spawn` 本身失败（命令不存在、没有执行权限……）。
    Spawn(String),
    /// 写 stdin 失败（多半是子进程已经死了，`BrokenPipe`）。
    WriteFailed(String),
    /// stdout 提前 EOF——子进程退出，或者主动关了 stdout。带上退出状态（能拿到
    /// 的话）和 stderr 尾巴，方便定位「是不是 npx 拉包失败」这类启动期问题。
    StreamClosed {
        exit_status: Option<i32>,
        stderr_tail: String,
    },
    /// 读一行响应等过了预算。
    Timeout { waited: Duration },
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportError::Spawn(m) => write!(f, "子进程起不来: {m}"),
            TransportError::WriteFailed(m) => write!(f, "写子进程 stdin 失败: {m}"),
            TransportError::StreamClosed {
                exit_status,
                stderr_tail,
            } => write!(
                f,
                "子进程 stdout 提前 EOF（退出码: {exit_status:?}）。stderr 尾巴:\n{stderr_tail}"
            ),
            TransportError::Timeout { waited } => write!(f, "等响应超时（{waited:?}）"),
        }
    }
}

impl std::error::Error for TransportError {}

/// 后台读线程往 channel 里发的东西：一行、或者流结束了。
enum StdoutEvent {
    Line(String),
    /// EOF——只发一次，发完这个之后线程退出（channel 发送端随之 drop，后续
    /// `recv_timeout` 立刻拿到 `Disconnected`，不用重复等一整个超时）。
    Eof,
}

/// stdio 子进程 + newline-delimited 读写。持有 `Child`——这正是红线 3 说的
/// 「不可序列化的活句柄」，只能住在 `McpRegistry` 这类进程内表里，绝不能进
/// 任何 atom（见 docs/INVARIANTS.md 红线 3、docs/MCP.md §「活句柄住 store 外」）。
pub struct StdioTransport {
    child: Child,
    stdin: ChildStdin,
    stdout_rx: Receiver<StdoutEvent>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
}

impl StdioTransport {
    /// spawn 子进程，起读 stdout / 读 stderr 两个后台线程。`envs` 是**追加**到
    /// 继承的父进程环境之上（`Command` 默认继承父环境，`.envs()` 只覆盖/新增
    /// 指定的 key，不清空）——MCP server 多半要用到 `PATH`（`npx` 得找到
    /// `node`），清空环境会直接让 spawn 出来的东西跑不起来。
    pub fn spawn(
        command: &str,
        args: &[String],
        envs: &[(String, String)],
    ) -> Result<Self, TransportError> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .envs(envs.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| TransportError::Spawn(e.to_string()))?;

        let stdin = child.stdin.take().expect("spawn 时已 Stdio::piped()");
        let stdout = child.stdout.take().expect("spawn 时已 Stdio::piped()");
        let stderr = child.stderr.take().expect("spawn 时已 Stdio::piped()");

        let (tx, stdout_rx) = mpsc::channel();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(l) => {
                        if tx.send(StdoutEvent::Line(l)).is_err() {
                            return; // 没人听了（transport 已经被 drop），收工。
                        }
                    }
                    Err(_) => break, // 当成 EOF 处理——底下统一发一次 Eof。
                }
            }
            let _ = tx.send(StdoutEvent::Eof);
        });

        let stderr_tail = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_TAIL_LINES)));
        let stderr_tail_writer = Arc::clone(&stderr_tail);
        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                let Ok(l) = line else { break };
                let mut tail = stderr_tail_writer.lock().unwrap();
                if tail.len() == STDERR_TAIL_LINES {
                    tail.pop_front();
                }
                tail.push_back(l);
            }
        });

        Ok(StdioTransport {
            child,
            stdin,
            stdout_rx,
            stderr_tail,
        })
    }

    /// 写一行（自动补换行）到子进程 stdin，立刻 flush——不攒批，子进程不会因为
    /// 「以为还有输入没写完」而卡住。
    pub fn write_line(&mut self, body: &[u8]) -> Result<(), TransportError> {
        self.stdin
            .write_all(body)
            .and_then(|_| self.stdin.write_all(b"\n"))
            .and_then(|_| self.stdin.flush())
            .map_err(|e| TransportError::WriteFailed(e.to_string()))
    }

    /// 阻塞读一行，最多等到 `deadline`。EOF / 超时都干净返回 `Err`，不 panic、
    /// 不永久挂——EOF 时顺带查一次子进程有没有已经退出，把退出码和 stderr 尾巴
    /// 一起带出去方便定位问题。
    pub fn read_line(&mut self, deadline: Instant) -> Result<String, TransportError> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match self.stdout_rx.recv_timeout(remaining) {
            Ok(StdoutEvent::Line(l)) => Ok(l),
            Ok(StdoutEvent::Eof) | Err(RecvTimeoutError::Disconnected) => {
                Err(self.stream_closed_error())
            }
            Err(RecvTimeoutError::Timeout) => Err(TransportError::Timeout { waited: remaining }),
        }
    }

    fn stream_closed_error(&mut self) -> TransportError {
        let exit_status = match self.child.try_wait() {
            Ok(Some(status)) => status.code(),
            _ => None,
        };
        let stderr_tail = self
            .stderr_tail
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        TransportError::StreamClosed {
            exit_status,
            stderr_tail,
        }
    }

    #[cfg(test)]
    pub(crate) fn child_id(&self) -> u32 {
        self.child.id()
    }
}

/// 子进程回收：**先杀后收尸**。`kill()` 只是让它死，`wait()` 才是真的把它从
/// 进程表里摘掉（不 `wait` 就是留一个僵尸）。目标可能已经自然退出——`kill`/
/// `wait` 在那种情况下分别返回「进程不存在」和「早就退出的状态」，都不是需要
/// 上报的错误，忽略即可（同 `agent-tools/src/shell.rs` 的 `kill_group` 处理
/// 方式）。
impl Drop for StdioTransport {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deadline(secs: u64) -> Instant {
        Instant::now() + Duration::from_secs(secs)
    }

    /// `cat` 把 stdin 原样回显到 stdout——用来验证 write/read 的换行分帧本身
    /// 是对的，不牵扯任何 JSON-RPC 语义。
    #[test]
    fn write_then_read_roundtrips_through_cat() {
        let mut t = StdioTransport::spawn("cat", &[], &[]).unwrap();
        t.write_line(b"hello").unwrap();
        assert_eq!(t.read_line(deadline(5)).unwrap(), "hello");

        t.write_line(b"world").unwrap();
        assert_eq!(t.read_line(deadline(5)).unwrap(), "world");
    }

    #[test]
    fn read_line_reports_stream_closed_when_process_exits_immediately() {
        let mut t =
            StdioTransport::spawn("sh", &["-c".to_string(), "exit 0".to_string()], &[]).unwrap();
        let err = t.read_line(deadline(5)).unwrap_err();
        match err {
            TransportError::StreamClosed { exit_status, .. } => {
                assert_eq!(exit_status, Some(0));
            }
            other => panic!("期望 StreamClosed，实际 {other:?}"),
        }
    }

    #[test]
    fn read_line_reports_stream_closed_when_stdout_closed_but_process_alive() {
        // 关掉 fd 1（stdout）之后睡一会——EOF 应该立刻可见，不用等 sleep 跑完。
        let script = "exec 1<&-\nsleep 5\n".to_string();
        let mut t = StdioTransport::spawn("sh", &["-c".to_string(), script], &[]).unwrap();
        let started = Instant::now();
        let err = t.read_line(deadline(5)).unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "EOF 应该在远小于 sleep 时长内被发现，实际 {:?}",
            started.elapsed()
        );
        assert!(
            matches!(err, TransportError::StreamClosed { .. }),
            "期望 StreamClosed，实际 {err:?}"
        );
    }

    #[test]
    fn read_line_times_out_promptly_when_nothing_arrives() {
        let mut t =
            StdioTransport::spawn("sh", &["-c".to_string(), "sleep 5".to_string()], &[]).unwrap();
        let started = Instant::now();
        let err = t
            .read_line(Instant::now() + Duration::from_millis(200))
            .unwrap_err();
        assert!(
            matches!(err, TransportError::Timeout { .. }),
            "期望 Timeout，实际 {err:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "超时应该在远小于原命令耗时内返回，实际 {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn drop_kills_and_reaps_child_leaving_no_zombie() {
        let t =
            StdioTransport::spawn("sh", &["-c".to_string(), "sleep 100".to_string()], &[]).unwrap();
        let pid = t.child_id();
        drop(t);

        // `wait()` 在 Drop 里已经跑完（同步、阻塞到收尸），这里应该立刻查不到
        // 这个 pid 了——用 `kill -0` 探测存在性，不发真信号。
        let status = Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .status()
            .expect("kill -0 本身应该能跑起来");
        assert!(
            !status.success(),
            "子进程应该已经被杀干净并收尸，pid {pid} 不该还存在"
        );
    }

    #[test]
    fn spawn_of_a_nonexistent_command_is_a_clean_spawn_error() {
        // `unwrap_err` 要 `T: Debug`——`StdioTransport` 持有 `Child` 等活句柄，
        // 故意不 derive `Debug`（红线 3 的活句柄不该被顺手序列化/打印），所以
        // 这里手动 match 取 `Err` 而不是 `unwrap_err`。
        match StdioTransport::spawn("this-command-does-not-exist-042", &[], &[]) {
            Err(err) => assert!(matches!(err, TransportError::Spawn(_))),
            Ok(_) => panic!("不存在的命令应该 spawn 失败"),
        }
    }
}
