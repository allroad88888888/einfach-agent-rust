//! `--ready-file` 的启动握手行为测试（058）：真的把这个 bin 起起来，验证
//! Java 那侧依赖的那份契约——**文件出现即端口可用**、pid 对得上、SIGTERM
//! 之后落盘再干净退出，以及不给这个参数时行为一字不变。
//!
//! 为什么是集成测试而不是脚本：`ready_file.rs` 的单测只能证明「写出来的
//! 字节是对的」，证明不了「文件里的端口真的是监听中的那个端口」——那需要
//! 一个真进程和一次真连接。`env!("CARGO_BIN_EXE_agent-server")` 让 cargo
//! 把这条验证钉进 `cargo test`，不用记得去跑某个脚本。
//!
//! 不引入 HTTP 客户端依赖：一次 `Connection: close` 的请求用 `TcpStream`
//! 手写十几行就够，跟这个 crate「依赖最小化」的取向一致（Cargo.toml 注释）。

#![cfg(unix)]

use std::{
    fs,
    io::{Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    thread::sleep,
    time::{Duration, Instant},
};

/// 起进程、等文件、等退出的统一上限。本地一般是百毫秒级，给足余量避免
/// 在负载高的机器上假红。
const DEADLINE: Duration = Duration::from_secs(30);
const TICK: Duration = Duration::from_millis(25);

static SEQUENCE: AtomicUsize = AtomicUsize::new(0);

/// 一次测试用的独占目录：providers.toml + 会话目录 + 子进程日志都放这里，
/// 不碰仓库工作目录（子进程的 `.agent-server-tools` 也会落在这里）。
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "agent-server-ready-handshake-{}-{sequence}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sessions")).unwrap();
        // key 是占位串，不是任何真 key：这条测试只走到「绑定端口 + 建会话」，
        // 一次 provider 请求都不发。
        fs::write(
            dir.join("providers.toml"),
            "[default]\nprovider = \"deepseek\"\n\n\
             [providers.deepseek]\napi_key = \"placeholder-not-a-real-key\"\n\
             base_url = \"http://127.0.0.1:1\"\nmodel = \"deepseek-chat\"\n",
        )
        .unwrap();
        Self { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// 起 `agent-server`：永远 `--port 0`（不跟别的测试抢端口），
    /// `ready_file` 为 `None` 时刻意一个 `--ready-file` 都不传。
    fn spawn(&self, ready_file: Option<&Path>) -> Child {
        let log = fs::File::create(self.path("server.log")).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_agent-server"));
        command
            .current_dir(&self.dir)
            .args(["--config", self.path("providers.toml").to_str().unwrap()])
            .args(["--sessions-dir", self.path("sessions").to_str().unwrap()])
            .args(["--port", "0"])
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log));
        if let Some(path) = ready_file {
            command.args(["--ready-file", path.to_str().unwrap()]);
        }
        command.spawn().unwrap()
    }

    fn log(&self) -> String {
        fs::read_to_string(self.path("server.log")).unwrap_or_default()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn wait_until<T>(what: &str, mut probe: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + DEADLINE;
    loop {
        if let Some(value) = probe() {
            return value;
        }
        assert!(Instant::now() < deadline, "等待{what}超时");
        sleep(TICK);
    }
}

/// 极简 HTTP/1.1：一次请求一条连接，`Connection: close` 之后读到 EOF。
fn http(port: u16, request_line: &str, body: Option<&str>) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let mut request = format!("{request_line} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    if let Some(body) = body {
        request.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        ));
    }
    request.push_str("\r\n");
    if let Some(body) = body {
        request.push_str(body);
    }
    stream.write_all(request.as_bytes()).unwrap();
    stream.flush().unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

fn field<'a>(record: &'a str, key: &str) -> &'a str {
    let start = record
        .find(key)
        .unwrap_or_else(|| panic!("就绪文件缺少 {key}：{record}"))
        + key.len();
    let rest = &record[start..];
    let end = rest.find([',', '}']).unwrap();
    rest[..end].trim_matches('"')
}

fn terminate(child: &mut Child) -> std::process::ExitStatus {
    Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    wait_until("子进程退出", || child.try_wait().unwrap())
}

#[test]
fn ready_file_publishes_the_listening_port_and_sigterm_exits_gracefully() {
    let fixture = Fixture::new();
    let ready_file = fixture.path("ready.json");
    let mut child = fixture.spawn(Some(&ready_file));

    let record = wait_until("就绪文件出现", || {
        fs::read_to_string(&ready_file).ok().filter(|r| r.ends_with('\n'))
    });

    // 契约三个字段都在，且 pid 就是这个子进程——Java 侧靠它交叉校验，
    // 确认读到的不是上一次启动留下的陈旧文件。
    assert_eq!(field(&record, "\"pid\":"), child.id().to_string(), "{record}");
    assert!(!field(&record, "\"version\":").is_empty(), "{record}");
    let port: u16 = field(&record, "\"port\":").parse().unwrap();
    assert_ne!(port, 0, "--port 0 必须发布操作系统实际分配的端口：{record}");
    // 原子发布不留半截文件：目录里只有 ready.json，没有临时文件残骸。
    let leftovers: Vec<_> = fs::read_dir(&fixture.dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| name.to_string_lossy().contains("ready.json"))
        .collect();
    assert_eq!(leftovers.len(), 1, "临时文件没清干净：{leftovers:?}");

    // 文件里的端口真的是监听中的那个端口：一次 chatid 幂等建会话 + 一次查询。
    let created = http(port, "POST /sessions", Some("{\"id\":\"probe-chat-1\"}"));
    assert!(created.starts_with("HTTP/1.1 201 "), "{created}");
    assert!(created.contains("\"outcome\":\"created\""), "{created}");
    let status = http(port, "GET /sessions/probe-chat-1", None);
    assert!(status.starts_with("HTTP/1.1 200 "), "{status}");

    let exit = terminate(&mut child);
    assert!(exit.success(), "SIGTERM 之后应干净退出，实际 {exit:?}");
    let log = fixture.log();
    assert!(log.contains("SIGTERM"), "{log}");
    assert!(log.contains("优雅关闭全部会话"), "{log}");
    // 落盘发生在退出之前——这正是 Java 侧敢用 `process.destroy()` 的理由。
    assert!(
        fixture.path("sessions/probe-chat-1.jsonl").is_file(),
        "SIGTERM 应在退出前把会话落盘：{log}"
    );
}

#[test]
fn without_the_flag_nothing_is_published_and_startup_is_unchanged() {
    let fixture = Fixture::new();
    let mut child = fixture.spawn(None);

    let log = wait_until("启动横幅", || {
        let log = fixture.log();
        log.contains("agent-server 监听 http://127.0.0.1:").then_some(log)
    });
    assert!(!log.contains("就绪文件="), "没给 --ready-file 就不该提它：{log}");
    assert!(
        fs::read_dir(&fixture.dir)
            .unwrap()
            .all(|entry| !entry.unwrap().file_name().to_string_lossy().contains("ready")),
        "没给 --ready-file 时不该写出任何就绪文件"
    );

    assert!(terminate(&mut child).success());
}
