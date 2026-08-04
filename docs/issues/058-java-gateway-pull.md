# 058 Java 网关升级：拉取 Rust → 产生 SSE + 进程生命周期 ← M9 终点

**里程碑** M9 · **依赖** 055/056/057 · **模型** sonnet（+ 主会话真机 dogfood） · **独测** —（终点靠真实全链）

把三块拼成企业能直接抄的形态：网关**拉取** Rust、**产生** SSE 给浏览器、**持有** Rust 进程
的生命周期、按 **chatid** 路由。接缝见 [INTEGRATION.md](../INTEGRATION.md)。

## 范围（`examples/java-gateway/`）

1. **改成拉取式**：删掉「代理上游 SSE」那条路（`bodyToFlux(DataBuffer)` 透传那套），换成
   循环 `GET /sessions/{chatid}/events/poll`（带 `Last-Event-ID` + `X-Poll-Wait-Ms`），
   把拿到的 `frames` **产生**成给浏览器的 SSE。**四个坑随之消失**（不缓冲/不压缩/超时/取消
   传播全是「代理流」才有的问题），并在 README 里如实记「为什么现在不强制 WebFlux 了」。
   **推荐长轮询**（wait 20–25s）：guard 全程持有，既避开「轮询间隔必须 < 宽限（5s）」的
   约束，又把空转请求降到最低（057 §注意）。
2. **chatid 路由**：浏览器带 chatid → 网关 `POST /sessions {id: chatid}`（幂等，055 三态）
   → 拿到 200/201 → 开始 poll。**网关必须保证 chatid 的归属**（用户 A 不能拿到用户 B 的
   chatid）——这条是**部署契约、代码解决不了**，要在 README 显著位置写清楚，并给出
   「chatid 含 uuid」或「网关侧 `user → chatid` 授权校验」两条出路。
3. **进程生命周期**：`@PostConstruct` 用 `ProcessBuilder` 起 `agent-server`
   （`--port 0` + `--sessions-dir` + 每次启动独占的 `--ready-file`），读成功 bind 后
   原子发布的 `{"port":…,"pid":…,"version":…}` 拿真实端口，并核对 pid 后建 WebClient；
   不解析人类启动日志。`@PreDestroy` `process.destroy()`（Unix SIGTERM → Rust 侧**所有会话
   落盘快照之后才退**，超时才强杀）。Rust bin 提供这份最小 ready-file/SIGTERM 协议。
   README 写清代价：二进制分发（按平台打进 resources + 解压 chmod）、进程僵尸兜底、
   子进程 stdout/stderr 接进 Java 日志。
4. **正常关闭发 `POST /cancel`**（显式出路），宽限是兜底不是主路。

## 验收

**代码侧**（本 issue）：`mvn -q package` 已通过。若未来在没有 JDK 的环境中维护这份
参考实现，仍按 [037](037-java-gateway.md) 的既定处置：如实标注构建验证缺席，不能伪造
验收结果。

**真机 dogfood**（主会话跑，M9 终点）：

- 起 Java 网关（它自己拉起 Rust 子进程）→ 浏览器带 chatid 打开 → 真实对话可用、
  SSE 正常流。
- **同一 chatid 重开页面** → 历史还在（055 幂等 + 恢复）。
- **关掉浏览器** → 宽限后在飞轮次被取消（057，不白烧 token）。
- **停掉 Java 进程** → Rust 子进程**一起干净退出**（`@PreDestroy` → SIGTERM → 落盘），
  `ps` 无残留；重启 Java → 同 chatid 历史仍在。

## 注意

- **没有 JDK 的环境不要伪造构建结果**。这是 037 立下的规矩，必须如实记录验证边界。
- **`examples/java-gateway` 不是依赖**（决策 13）：不发 Maven、不跟版、README 第一句仍是
  「拷走改，别当依赖」。本 issue 只是让这份参考实现跟上新形态。
- **providers.toml 只读不印不提交**：真机 dogfood 要读它拿 key，任何输出只出长度/状态。
- **红线 8**：Rust 子进程绑 loopback（默认），网关不把它暴露出去。
- 真机若捞到新问题 → 单列新 issue，不塞进本 issue 硬修（049/050 的先例）。
- 收工验证前台跑完（WORKFLOW §四 -1）。

## 真机全链 dogfood（主会话跑完 · 2026-08-04 · **M9 收官**）

真 Java 网关（OpenJDK 21.0.11 + Spring Boot 3.3.4，`mvn package` 已构建验证）+ 真 deepseek
上游 + `curl --noproxy '*'`（本机有系统代理，不加会假 502）。**全程没有手工起 Rust**——
Rust 子进程由网关自己拉起，这正是本 issue 要验的东西。

**一、网关拉起 Rust 子进程 + ready-file 握手**

```
[agent-server] agent-server 监听 http://127.0.0.1:49611（provider=deepseek …）
[agent-server] 就绪文件=…/agent-server-ready-9408…/ready.json（成功 bind 后原子发布实际端口）
Netty started on port 8080 (http)
Started AgentGatewayApplication in 1.055 seconds
```

`--port 0` 让操作系统分配（49611），网关经 ready 文件拿到它——**没有解析 stderr 文本**。
子进程的 stdout/stderr 被转发进 Java 日志（`[agent-server]` 前缀），运维只看一份日志。

**二、chatid 幂等路由（经网关，055 的三态）**

```
POST /agent/sessions {"id":"gw-chat-1"} → 201 {"id":"gw-chat-1","outcome":"created"}
再来一次                                → 200 {"id":"gw-chat-1","outcome":"existing"}
```

**三、拉取式 → 产生 SSE（本 issue 的核心）**

`GET /agent/sessions/gw-chat-1/events` 拿到 **67 帧**，形状：

```
id:1
data:{"agent":"root","event":{"type":"agent_tree","data":{"nodes":[{"id":"root",…,"activity":"Thinking"}]}}}

id:3
data:{"agent":"root","event":{"type":"thinking_delta","data":"天空"}}
```

**`id:` 游标保留**（客户端重连语义跟直连 server 时一致）、**逐帧到达**（`thinking_delta`
一个词一帧，没有被攒到块边界）——「产生 SSE 而不是代理 SSE」那四个坑（不缓冲/不压缩/
超时/取消传播）在这条链上**结构上不存在**。真实对话可用，模型答完整：

> 天空之所以是蓝色的，是因为太阳光进入大气层后，波长较短的蓝光比波长较长的红光更容易被
> 空气分子向四面八方散射（瑞利散射）……

**四、停 Java → Rust 一起干净退出**

```
kill <java pid> → 网关已退 → Rust 已退（干净，无孤儿）
会话文件：gw-sessions/gw-chat-1.jsonl（1812 字节）
```

`@PreDestroy` → SIGTERM → Rust 侧「所有会话落盘快照之后才退出」。**一个部署单元**的承诺
兑现：管 Java 的生死就等于管住了整条链。

**五、跨进程恢复**

全新 Java 进程 + 全新 Rust 子进程，同 chatid：

```
POST /agent/sessions {"id":"gw-chat-1"} → 200 {"outcome":"recovered"}
```

会话文件里确有上一轮：`{"label":"user_input","turn_id":1,…}` + `{"label":"provider_done",…}`。

**一条需要说清的观察（不是 bug）**：重启后拉 SSE **拿不到上一轮的帧**。这是**设计如此**——
恢复的是**消息历史**（在 store 里、随 `.jsonl` 落盘），不是**事件流**（ring 是传输层的
内存重放缓冲，从不持久化）。`Last-Event-ID` 补发只在同一进程生命周期内有效；跨重启
「历史还在」由 `outcome:recovered` + store 里的消息保证，模型接着聊时看得到上文。
这条写在这里是因为**下一个人很容易把它误读成缺陷**。

## 实做记录（代码部分 · 2026-08-04）

四件事都落地了，Rust 侧只多了一个 flag，剩下全在 `examples/java-gateway/`。**没有新增
协议、没有新增真值源**：网关拉的是 056 的端点、握手读的是 bind 之后写的一个文件、
chatid 走的是 055 的幂等三态。

### 建了什么

| 文件 | 行 | 干什么 |
|---|---|---|
| `agent-server-bin/src/cli.rs` | 197 | 改：第四个 flag `--ready-file <path>`（`--flag value` / `--flag=value` 两种写法照旧），`HELP` 补一行 |
| `agent-server-bin/src/ready_file.rs` | 193 | 新：就绪文件的原子发布 + 四条单测（内容、无临时文件残骸、陈旧文件拒绝、并发只成一个） |
| `agent-server-bin/src/run.rs` | 175 | 改：`bind` 成功之后、启动横幅之前发布就绪文件；发布失败非零退出 |
| `agent-server-bin/tests/ready_file_handshake.rs` | 205 | 新：真起这个 bin 的行为测试（见下「验证」） |
| `java-gateway/.../runtime/AgentServerProcess.java` | 181 | 新：`@PostConstruct` 起子进程 + 等就绪文件 + 校 pid；`@PreDestroy` SIGTERM |
| `java-gateway/.../proxy/AgentSessionClient.java` | 75 | 新：`POST /sessions {id}`、`GET /events/poll`、`POST /cancel` 三个上游调用 |
| `java-gateway/.../proxy/AgentSseController.java` | 117 | 改：从「代理上游 SSE」换成「拉取 → 产生 SSE」，含游标推进与显式 cancel |
| `java-gateway/.../proxy/ChatSubscribers.java` | 37 | 新：本网关每个 chatid 还剩几条浏览器连接——只用来决定要不要发显式 cancel |
| `java-gateway/.../proxy/PollResponse.java` | 15 | 新：`{"frames":[{"id","event"}],"next"}` 的线上形状 |
| `java-gateway/.../config/AgentRuntimeProperties.java` | 59 | 新：命令/工作目录/providers.toml/会话目录/key 变量名/超时/`poll-wait-ms`，全是非机密项 |
| `java-gateway/.../proxy/AgentProxyController.java` | 65 | 改：只剩「其余短请求」转发，不再替浏览器生成随机 session id |
| `java-gateway/README.md` | 255 | 改：见下「README 改了哪些」 |

`HopByHopHeaders.java`（49）与 `WebClientConfig.java`（23）基本没动，后者只是不再设
`responseTimeout`——**给这个 client 配一个比长轮询更短的响应超时会把正常空等判成失败**。

### 1. `--ready-file` 的握手协议

**形状**（一行 UTF-8 JSON，带结尾换行）：

```
{"port":43127,"pid":12345,"version":"0.1.0"}
```

**写入时机**：`AgentServer::bind(addr).await` 返回之后、打印启动横幅之前，端口取
`bound.local_addr().port()`——所以 `--port 0` 时文件里是操作系统实际分配的那个端口。
**发布失败 = 非零退出**，不会出现「进程在跑但父进程永远等不到文件」这种半死不活的状态。

**为什么是 `hard_link` 不是 `rename`**（issue 描述里写的是 rename，实做换掉了）：两者都
原子，但 `rename` 在 Unix 上**会静默覆盖已存在的目标**，而 `hard_link` 要求目标不存在。
后者顺手把生命周期契约钉死：父进程必须为每次启动给一个新路径，**上一次启动留下的陈旧
文件不可能被当成这一次的成功**（`reject_existing_target` 先挡一道，`hard_link` 是第二道，
并发场景下只有一个发布者能赢——`concurrent_publishers_cannot_replace_each_others_record`
钉的就是这条）。临时文件与目标同目录，保证同一个文件系统。

`pid` 给父进程跟 `Process.pid()` 交叉校验用；`version` 留给部署期做兼容判断。
**不给这个 flag 时行为一字不变**——不写文件、横幅里不提它，有测试钉住。

### 2. 网关改拉取式：代理流的四个坑随之消失

`bodyToFlux(DataBuffer)` 透传上游 SSE 那条路删掉了，换成一个自己的循环：

```java
private Flux<ServerSentEvent<String>> pollForever(String chatid, PollCursor cursor, HttpHeaders h) {
    return Flux.defer(() -> sessions.poll(chatid, cursor.lastEventId(), h)
                    .flatMapMany(response -> {
                        cursor.advance(response.next());          // next 直接当下次的 Last-Event-ID
                        return Flux.fromIterable(response.framesOrEmpty())
                                .map(AgentSseController::toSse);  // 产生给浏览器的 SSE
                    }))
            .repeat();
}
```

上游请求带 `Last-Event-ID: <游标>` + `X-Poll-Wait-Ms: 25000`（**长轮询**：057 的 guard
全程持有，既避开「短轮询间隔必须 < 5s 宽限」的约束，又把空转请求降到最低）。

**`next` 不加一**：`PollCursor.advance` 只做「拿服务端算好的值」，并断言它不为 null、
不倒退。ring 只回 `id > Last-Event-ID`，自己 +1 会跳帧；空批时服务端原样回传入游标，
所以这条路径不需要任何特判。

浏览器那一跳的协议**一行没改**：`ServerSentEvent` 的 `id` 用帧 id、`data` 用整个 Frame
信封 JSON，跟 Rust 原生 SSE 逐字节同形，`EventSource` 的自动重连与 `Last-Event-ID` 照旧。

### 3. chatid 路由

每条浏览器 SSE 连接先发一次 `POST /sessions {"id": chatid}`（055 幂等三态：`existing` /
`recovered` / `created` → 200/200/201），拿到之后才开始 poll。网关不看状态码分支——三态
对它是同一件事「这个 chat 现在可用」，区别只在 Rust 侧建没建、恢没恢复。

**部署契约写进 README 显著位置**（现在是正文第一节，标题就叫「Deployment contract you
must satisfy first: chatid ownership」）：chatid 是身份边界，**猜到别人的 chatid 就能接上
别人的会话**，两条出路——chatid 含 uuid，或网关侧做 `user → chatid` 授权校验。
这条**代码解决不了**，README 明写「no amount of code in this example can fix it for you」。

### 4. 进程生命周期 + 显式 cancel

`@PostConstruct`：建独占临时目录 → `ProcessBuilder(bin, --config, --sessions-dir, --port 0,
--ready-file)` → `redirectErrorStream(true)` + 一条 daemon 线程把子进程输出接进 Java 日志
→ 轮询就绪文件（同时检查子进程是否已退出，退了立刻报错而不是干等到超时）→ **校验
`ready.pid == child.pid()`** → 用文件里的端口拼 `http://127.0.0.1:<port>/`。全程不解析横幅。

`@PreDestroy`：`process.destroy()`（Unix = SIGTERM → Rust 侧 `close_all` 落盘快照之后才退）
→ `waitFor(shutdown-timeout)` → 超时才 `destroyForcibly()`。

**显式 cancel 是本次新加的一条**（issue §范围 4）：浏览器这条流结束时（关 tab = Reactor
的 CANCEL 信号），网关不只是走开，而是主动发 `POST /sessions/{chatid}/cancel`。
`ChatSubscribers` 是为它服务的——**只在本网关这个 chatid 上最后一条连接走掉时才发**，
否则关掉两个 tab 中的一个就会取消整个 chat 在飞的轮次。

这份计数**不是 Rust 订阅计数的副本**：Rust 那一份还包含直连 SSE 的客户端和别的网关实例，
并且仍然是取消的权威（引用计数 → 宽限 → `handle.cancel()`）。本地这份只回答「本进程要不要
主动说一声」。代价 README 里如实写了：最后一条连接在轮次中途断掉、浏览器随即重连时，
轮次已经被取消而不是还在跑——这是「把显式出路当主路」换来的。

### README 改了哪些

1. **删掉「本机无 JDK、构建验证缺席」那段**，换成「Build verification」：`mvn -q package`
   已用 OpenJDK 21 + Maven 3.9.15 真跑通过（Spring Boot 3.3.4 支持 17–21，编译目标 17）。
   同时保留 037 的规矩：**将来在没有 JDK 的环境里维护这份代码，仍然如实标注缺席，不许
   伪造构建结果**。
2. **chatid 归属契约提到正文第一节**（原来在文档中后段）。
3. 新增「Closing a stream: explicit cancel, with grace as the safety net」——显式 cancel、
   `ChatSubscribers` 为什么存在、Rust 宽限兜什么、以及上面那条代价。
4. 「为什么现在不强制 WebFlux 了」重写成**逐条点名代理流的四个坑**（不缓冲 / 不压缩 /
   超时放开 / 取消传播），并说明每一个都只属于**转发**流、不属于**产生**流；
   MVC 也能照这个协议实现，唯一还成立的约束是别为每条浏览器连接占一个请求线程。
5. 进程生命周期的代价单独列成三条：**二进制分发**（按平台打进 resources → 解压到私有目录
   → `chmod +x`，从 jar 里解出来的文件没有执行位）、**僵尸兜底**（`destroy` →
   `waitFor` → `destroyForcibly` 只覆盖 JVM 正常退出，`kill -9` JVM 时子进程会活下来，
   要靠 systemd cgroup / Pod 生命周期兜）、**子进程输出**（那条读取线程是必需的，
   不读会把管道缓冲写满卡住子进程）。
6. 验证步骤补 `curl --noproxy '*'`：本机有 `http_proxy` 时，打 loopback 的 curl 会被
   送进代理然后报一个跟网关无关的 502。另补 macOS 的 `/usr/bin/java` stub 坑
   （必须 `export JAVA_HOME=/opt/homebrew/opt/openjdk@21` 这类真 JDK）。
7. Source map 补 `ChatSubscribers` / `HopByHopHeaders` / `PollResponse`。

### 验证

**`--ready-file` 的行为测试**（`tests/ready_file_handshake.rs`，两条，`#![cfg(unix)]`）：
单测只能证明「写出来的字节是对的」，证明不了「文件里的端口真的是监听中的那个端口」——
那需要一个真进程和一次真连接。所以这条走 `env!("CARGO_BIN_EXE_agent-server")` 真把 bin
起起来，**不引入 HTTP 客户端依赖**（`TcpStream` 手写一次 `Connection: close` 的请求，
跟这个 crate 依赖最小化的取向一致）：

1. `ready_file_publishes_the_listening_port_and_sigterm_exits_gracefully`：`--port 0
   --ready-file <tmp>` → 文件出现 → `pid` 等于子进程 pid、`version` 非空、`port != 0`
   → 目录里只有 `ready.json`（无临时文件残骸）→ 用文件里的端口 `POST /sessions
   {"id":"probe-chat-1"}` 得 **201 + `"outcome":"created"`**、`GET /sessions/probe-chat-1`
   得 **200**（端口是对的，且 chatid 幂等三态在）→ SIGTERM → **退出码 0**、日志有
   「SIGTERM」「优雅关闭全部会话」、**`sessions/probe-chat-1.jsonl` 已落盘**。
2. `without_the_flag_nothing_is_published_and_startup_is_unchanged`：不给这个 flag →
   横幅照常、横幅里不提就绪文件、目录里没有任何 ready 文件 → SIGTERM 干净退出。

```
running 2 tests
test ready_file_publishes_the_listening_port_and_sigterm_exits_gracefully ... ok
test without_the_flag_nothing_is_published_and_startup_is_unchanged ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

**Rust 门禁**（本机主 target 被别的会话占着，用独立 target 目录跑）：

```
cargo clippy -p agent-server-bin --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 7.72s
bash scripts/check-invariants.sh --all
    红线检查通过
cargo test -p agent-server-bin -p agent-server
    总计 149 passed; 0 failed
```

**Java 门禁**（`JAVA_HOME=/opt/homebrew/opt/openjdk@21`，OpenJDK 21.0.11 + Maven 3.9.15）：

```
[INFO] --- compiler:3.13.0:compile (default-compile) @ agent-gateway ---
[INFO] Compiling 10 source files with javac [debug parameters release 17] to target/classes
[INFO] --- jar:3.4.2:jar (default-jar) @ agent-gateway ---
[INFO] Building jar: .../target/agent-gateway-0.0.0-reference.jar
[INFO] --- spring-boot:3.3.4:repackage (repackage) @ agent-gateway ---
[INFO] Replacing main artifact ... with repackaged archive, adding nested dependencies in BOOT-INF/.
[INFO] BUILD SUCCESS
[INFO] Total time:  3.273 s
```

`mvn -q package` 同样 exit 0（`-q` 下无输出）。**这是编译与打包的结果，不证明任何运行时
行为**——README 里也是这么写的。

**一条环境噪声，不是缺陷**：这台机器同时跑着别的会话的构建（load average 17+），
`cargo test` 里**第一次 exec 刚链接出来的 25MB 二进制**会慢到 15 秒左右（第二次 exec 只要
30ms）。从 python 连起六次同一个二进制，就绪文件稳定在 **7–8ms** 出现。已排除
`ready_file::publish` 的 fsync（去掉 `sync_all` 后耗时不变），是 macOS 侧首次 exec 的
代价。测试的等待上限取 30s 就是为了不在这种负载下假红。

### 明确未做

**真机全链 dogfood 待主会话**（它有 playwright + 真实 key）：起 Java 网关（它自己拉起 Rust
子进程）→ 浏览器带 chatid 打开真实对话 → 同一 chatid 重开页面看历史 → 关掉浏览器看在飞
轮次被取消 → 停掉 Java 看 Rust 子进程一起干净退出、`ps` 无残留、重启后历史仍在。
本 issue 只做到「代码 + 构建验证」这条线。

真机若捞到新问题按注意事项单列新 issue，不塞回本 issue。
