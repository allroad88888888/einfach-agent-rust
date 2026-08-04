# 055 `chatid` 幂等 getOrCreate + id 白名单校验

**里程碑** M9 · **依赖** 059（先修泄漏，否则每个 chat 一个 session 会放大它） · **模型** sonnet · **独测** —

会话身份从「server 生成的随机 id」改成「业务侧的 chatid」，并把它做成**幂等**的：
拿 chatid 查历史，有就接上、没有就建。接缝见 [INTEGRATION.md](../INTEGRATION.md) §三。

## 范围

1. **`POST /sessions` 接受客户端指定 id**（`agent-server/src/http/routes/sessions.rs`）：
   请求体加一个可选字段（如 `id`）。给了就用它当 `SessionId`，没给就退回现有的
   `state.generate_id()`（**向后兼容，现有调用方一行不改**）。
2. **幂等三态**：

   | chatid 状态 | 行为 | 状态码 |
   |---|---|---|
   | registry 里活着 | 直接接上（**不新建、不清空历史**） | 200 |
   | registry 没有、磁盘有 `<sessions-dir>/<chatid>.jsonl` | **恢复**（走既有 recover 路） | 200 |
   | 都没有 | 新建 | 201 |

   「查历史」**不新造机制**：磁盘上有那个文件就是有历史，恢复走的是 kill-9 重启那条既有路
   （`agent_runtime::recover`）。响应体要能让调用方区分这三态（至少 200/201 + 一个字段）。
3. **id 白名单校验**（安全，见 §注意）：只允许 `[A-Za-z0-9_-]`、限长 ≤128，
   **不合规 400 直接拒，不做 sanitize**。校验点在**接受 id 的入口**（一处），不是在拼路径
   的地方（那可能有好几处）。
4. `POST /sessions` 成功后仍然 `let _ = state.hub_for(&id);`（现有行为，理由见
   `sessions.rs` 注释：不然「先 POST input 好几轮、稍后才首连」会在 hub 存在前丢事件）。

## 验收（可判定）

- 同一个 chatid 连着 POST 两次 → 第二次 **200 不是 201**，且**历史还在**（第一次的对话
  内容仍能读到，没被清空）。
- 进程重启（或 close 之后）用同一个 chatid POST → **200 + 历史恢复**（断言恢复出来的消息
  条数/内容跟重启前一致），走的是既有 recover。
- 全新 chatid → **201**。
- **路径穿越拒绝**：`id` 为 `../../etc/passwd`、`a/b`、`..`、超长串 → **400**，且
  **文件系统上没有任何文件被创建**（这条要断言，不能只断言状态码）。
- 不给 `id` 的老调用方 → 行为跟现在一字不差（生成 id、201）。

## 注意

- **安全点一（路径穿越）**：chatid 由客户端给、又拼进文件名。**不做 sanitize 只做拒绝**
  ——悄悄改写会让两个不同 chatid 撞进同一个会话文件，比拒绝更坏（静默串会话）。
- **安全点二（chatid 即身份）**：server 无鉴权是 by design，但 chatid 成为身份之后
  **猜到别人的 chatid 就能接上别人的会话**。这条**代码解决不了**，是部署契约——
  写进网关文档（058）：网关必须保证 chatid 的归属，推荐含不可猜部分（uuid）。
  本 issue 只需在 `POST /sessions` 的文档注释里点明这个契约。
- **红线 8 邻近**：`sessions-dir` 下的文件名来自外部输入，白名单是唯一防线。
- **不要**顺手做多租户 / `X-Agent-Tenant-Id` 鉴权——那是未排期项（ARCHITECTURE §无鉴权
  ≠ 无身份），本 issue 只做身份的**幂等取用**。
- 收工验证前台跑完（WORKFLOW §四 -1）。

## 实做记录（完成 · 2026-08-04）

接缝说的「几乎全是白拿」是真的：`SessionId` 本来就是 `Arc<str>`、`default_sessions_dir`
本来就落 `<dir>/<id>.jsonl`、恢复本来就是 `registry.open` → `agent_runtime::recover`。
**没有新增 atom / 事件 / 恢复路径 / 存储格式**，改动全部在 HTTP 这一层。

### 建了什么

| 文件 | 行 | 干什么 |
|---|---|---|
| `agent-server/src/http/routes/sessions.rs` | 167（+38 码/+38 文档） | 改：`CreateSessionRequest` 加可选 `id`；`create` 变成幂等 getOrCreate（三态 + 状态码）；`is_valid_client_session_id` 白名单；`create` 的文档注释写下两个安全点（含**安全点二这条代码解决不了的部署契约**） |
| `agent-server/src/http/config.rs` | 221（+7） | 改：`SessionTemplate::default_session_path(&id)`——把「这个 id 的默认 jsonl 是哪个文件」从 `open_spec` 里**只读地**抽出来一份，好在 `open` 之前先看一眼。**不建目录**（建目录仍然只发生在 `open_spec`），无效请求因此不会留下空目录 |
| `agent-server/tests/http_chatid_sessions.rs` | 174 | 新：四条验收各一条测试（活会话重连 / 关闭后恢复 / 无效 id 拒绝 / 老调用方不受影响） |

`state.rs` 一行没动——三态判定要的两件事（`registry()`、`template()`）它早就有了。

### 三态怎么判定的：一次 registry 查询 + 一次「open 之前」的文件存在性

```rust
let has_persisted_history = client_supplied_id
    && session_path.clone().or_else(|| state.template().default_session_path(&id)).is_some_and(|p| p.is_file());
let outcome = match state.registry().get(&id) {
    Some(SessionQuery::Alive(_)) => Existing,                       // 200，原样接上
    Some(SessionQuery::Dead { .. }) | None => {                     // 起一个 actor
        state.registry().open(spec)?;                               // 内部就是既有 recover 路
        if has_persisted_history { Recovered } else { Created }     // 200 / 201
    }
};
```

**唯一有讲究的一处是顺序**：文件存在性必须在 `open` **之前**取——`agent_runtime::recover`
之后（或者哪怕只是 `Jsonl` 的 IO 线程刚建好文件之后）再看，答案永远是「在」，
`created` 和 `recovered` 就再也分不开了。`Dead` 那一档跟 `None` 走同一条路：registry 的
`open` 本来就允许「已死的旧 entry 不挡道，先摘掉再起新的」，这里不另造判断。

「查历史」**没有新机制**：磁盘上有那个文件就是有历史，接上历史这件事完全由既有的
`recover` 完成——本 issue 只是把「文件在不在」这个事实**翻译成 HTTP 状态码**。

响应体形状（`outcome` 只在调用方指定了 `id` 时才出现，旧请求的 `{"id":...}` 一字不变）：

```
POST {"id":"customer_42-chat"} → 201 {"id":"customer_42-chat","outcome":"created"}
再来一次（还活着）           → 200 {"id":"customer_42-chat","outcome":"existing"}
close/重启之后再来           → 200 {"id":"customer_42-chat","outcome":"recovered"}
POST {}                      → 201 {"id":"sess-<pid>-<n>"}          ← 没有 outcome 字段
```

### 白名单校验点：收下 id 的那一处，不是拼路径的那几处

`crates/agent-server/src/http/routes/sessions.rs:88`，在 `SessionId::from(id)` **之前**——
往下一步这个字符串就成了 `SessionId`，而 `SessionId` 会被拼进 `<sessions-dir>/<id>.jsonl`
和 `tools_root/<id>/` 两处。全仓 `registry().open(..)` 的调用点只有这一个（生产码路径），
所以「入口只有一处」不是约定，是可 grep 判定的事实。

规则：非空、`[A-Za-z0-9_-]`、≤128 字节，其余一律 400。点号不在白名单里，于是 `.`/`..`
连同 `/`、`\`、NUL、非 ASCII 被同一条规则挡掉——不用维护「危险序列」黑名单（漏一个就是
漏一个）。**不 sanitize**：把 `a/b` 洗成 `a_b` 会让两个不同的 chatid 撞进同一个会话文件，
静默串会话比拒绝更坏。

### 路径穿越那条测试：断言的是「沙箱整棵树逐项不变」，且反向验证过它真的会红

只断言 400 是不够的（验收明写「文件系统上没有任何文件被创建」要真断言）。做法：
把 `sessions-dir` 和 `tools-root` 都放进私有沙箱的**第三层**（`sandbox/a/b/`），于是
`../../` 这种穿越即使成功也仍然落在沙箱内，一次递归遍历（`tree_under`）比较前后快照，
**内部污染和向上逃逸一起抓**。反过来，直接写 `!tools_root.join(id).exists()` 是错的：
`TMPDIR=/tmp` 的机器上 `/tmp/x/../../etc/passwd` 会正规化成真实存在的 `/etc/passwd`，
那条断言会在没有任何 bug 时假失败。

**反向验证**（把校验临时改成 `if false` 让坏 id 放行）：

```
---- invalid_chatids_are_rejected_before_creating_any_session_file stdout ----
panicked at crates/agent-server/tests/http_chatid_sessions.rs:99:9:
assertion `left == right` failed: 无效 id 不得在文件系统上留下任何痕迹：id="../../etc/passwd"
  left: ["a", "a/b", "a/b/sessions", "a/b/tools", "a/etc", "a/etc/passwd", "a/etc/passwd.jsonl"]
 right: ["a", "a/b", "a/b/sessions", "a/b/tools"]
```

多出来的三项就是这次穿越的战果：`a/etc/passwd/` 是工具监狱目录（`tools_root.join(id)`
向上逃了两层），`a/etc/passwd.jsonl` 是**会话文件本体**——两条拼路径的路各中一发。
这条穿越是实打实的，不是理论风险。

顺带记一笔：加固之前那版测试断言的是「两个目录各自为空」，它能抓住 `a/b`（在
`tools-root` 里造子目录），**却抓不住 `../../etc/passwd`**——那个 id 直接逃出了被检查的
两个目录，`read_dir` 看过去仍然是空的，测试照样绿。加固就是为了这一条。

### 验证（前台跑完，独立 target 目录避开并发会话的 cargo 锁）

`cargo test -p agent-server`——36 个测试二进制，**132 passed / 0 failed**，零 warning：

```
     Running tests/http_chatid_sessions.rs
running 4 tests
test omitting_chatid_keeps_the_legacy_generated_id_and_created_status ... ok
test invalid_chatids_are_rejected_before_creating_any_session_file ... ok
test repeated_chatid_reattaches_to_the_live_session_without_clearing_history ... ok
test closed_chatid_recovers_history_from_its_default_session_file ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
```

`cargo clippy -p agent-server --all-targets -- -D warnings`：

```
    Checking agent-server v0.1.0 (/Volumes/work/self/einfach-agent-rust/crates/agent-server)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.96s
```

`bash scripts/check-invariants.sh --all`：

```
红线检查通过
规则与理由：docs/INVARIANTS.md
```

红线 9：改到的三个文件分别 167 / 221 / 174 行，都在 300 以内，没有需要拆的。

### 没做什么

多租户 / `X-Agent-Tenant-Id` 鉴权一行没碰（未排期项）。安全点二（chatid 即身份、归属
由网关保证）代码解决不了，只写进了 `create` 的文档注释——它是部署契约，058 的网关
README 还要再写一遍。
