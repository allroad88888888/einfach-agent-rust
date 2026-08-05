# 085 HTTP 上行：收附件、上传、再进 actor

**里程碑** M11 · **依赖** [081](081-image-user-input.md) + [084](084-transport-files-upload.md) · **模型** sonnet · **独测** ✅ · **状态** 完成

**先读 [docs/IMAGES.md](../IMAGES.md) §八「上传发生在哪」。那条最容易放错，
而且放错了不报错——只表现为多 agent 不并行了。**

## 范围

1. **`POST /sessions/:id/input` 收附件**。现在是 `InputRequest { text: String }`
   （`agent-server/src/http/routes/input.rs:21`）。加图片字段。
   **不带图时请求体逐字节还是 `{"text":"..."}`**，老客户端一个字不用改
   （065/067 都是这么做的，照办）。

2. **顺序：先上传，成功了再 dispatch。**
   路由层拿到附件 → 调 [084](084-transport-files-upload.md) 的上传 → 拿到 `ms://` 引用 →
   带着引用把 `Command` 送进 actor。

3. **`agent-server` 的 `Command::Input(String)`**（`command.rs:42`）要能带图片引用。

4. **endpoint 与 api_key 从哪来**：`SessionTemplate`（`bootstrap.rs` 拼的那份）里有
   `provider`/`endpoint`/`api_key`/`model` 四个字段。**动手第一件事是确认
   `AppState` 拿不拿得到**——拿不到就照最小改动接一条，**并把这条路径写进实做记录**。

## 不许放的三个位置（是硬约束，不是偏好）

| 位置 | 为什么不行 |
|---|---|
| core / store | 红线 7，不许做 IO |
| adapter 的 `encode` | `Provider` trait 全部方法是纯函数，`agent-providers/src/lib.rs` 开头写着「零 IO」 |
| `provider_call::start` | 它跑在 **actor 线程**上，而拆成 start/finish 两半的全部意义就是「只做能在 actor 线程上做完的部分」（029 的并行就是这一刀）。往里加一次网络上传 = 把所有 agent 的并行掐死。而且上传只需一次，不是每轮 |

## 验收（可判定）

1. **不带图 = 老路逐字节不变**：`{"text":"hi"}` 走完整条路，行为与本 issue 之前
   完全相同（要有断言）。
2. **带图 → 引用进了 store**：POST 一张图 → 那一轮的历史里有 `ContentBlock::Image`，
   `reference` 是 `ms://` 开头且非空。
3. **上传失败不留残骸**：让上传返回 500 → 端点 400、**会话历史一条都没多**、
   会话仍然可用（下一句纯文本输入照常跑完）。**这条比成功路径重要。**
4. **actor 没被卡住**：上传期间**另一个会话**的普通输入照常跑完。
   这条挡的就是「把上传写进 `provider_call::start`」那个错误落点。
5. **错误可读**：文件太大 / key 不对 / provider 拒了，三种在报文里分得开，
   且**都不含 api_key**。

## 突变验证（必做）

- 把「上传失败也照样 dispatch」打开 → 第 3 条必须变红。
- **把上传搬进 actor 线程 → 第 4 条必须变红。** 这条要真跑红：
  它是本 issue 唯一一个「不报错、只表现为性能塌陷」的失败形态。

## 注意

- 红线 8：不新开监听端口，就在现有 `bind` 上加字段。
- **providers.toml 只读不印不提交**，任何输出只出长度/状态。
- 真机验证加 `--noproxy '*'`（本机代理会让 localhost 假 502，M9 踩过还误判成
  「vite 代理坏了」）。
- 真机若捞到新问题 → **单列新 issue**，不塞进本 issue 硬修（049/050/060/078 的先例）。
- 收工验证前台跑完，含 `--features ts`。
- **本次实作确认**：route 可以经 `AppState.templates` 取得完整 `SessionTemplate`；上传必须在
  dispatch 前、actor 外完成。把它延后到 actor 会令同一 session 队列中的纯文本等待上传网络时间。

---

## 实做记录（完成 · 2026-08-04）

- `SessionTemplate` 经 `AppState.templates` 由 bootstrap 装配并传到 HTTP input route；路由在
  `spawn_blocking` 中调用 `agent-transport::Client::upload_image`，全部成功后才把完整 `ms://`
  引用随 `Command::Input` 送入 session actor。core、store、provider encode 与
  `provider_call::start` 均未加入 IO。
- HTTP 输入体新增可选 `images`。纯文本序列化仍逐字节是 `{"text":"hi"}`；有图成功后历史的
  本轮 `UserInput` 含完整不透明 `ContentBlock::Image`。测试还断言上传 500 映射为 400、不留下
  history、后续纯文本可继续完成，以及文件过大、401、服务商拒绝三种报文各自可辨且不含 key。
- `cargo test -p agent-server --test http_image_input --test http_image_input_errors`：5 passed，0 failed。
  两个独立 session 的慢上传不阻塞普通输入；另有同一 session 的队列测试，证明上传等待本身也未
  被错误放进 actor。

### 突变验证：先红后恢复

临时令上传返回 500 后仍然 dispatch，运行
`cargo test -p agent-server --test http_image_input_errors rejected_upload_leaves_no_history_and_error_kinds_are_readable_without_the_key -- --exact`。目标的「上传失败不得留下历史」断言首先红为（实现已恢复）：

```text
thread 'rejected_upload_leaves_no_history_and_error_kinds_are_readable_without_the_key' (42625253) panicked at crates/agent-server/tests/http_image_input_errors.rs:35:5:
assertion `left == right` failed: 上传 500 必须在 HTTP 边界变成 400：
  left: 202
 right: 400
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test rejected_upload_leaves_no_history_and_error_kinds_are_readable_without_the_key ... FAILED

failures:

failures:
    rejected_upload_leaves_no_history_and_error_kinds_are_readable_without_the_key

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.03s

error: test failed, to rerun pass `-p agent-server --test http_image_input_errors`
```

临时把上传改为 actor 内的预上传图片，并使该路径延迟 700ms，运行
`cargo test -p agent-server --test http_image_input slow_upload_leaves_its_session_actor_available_until_the_reference_is_ready -- --exact`。目标并行护栏首先红为（实现已恢复）：

```text
thread 'slow_upload_leaves_its_session_actor_available_until_the_reference_is_ready' (42633861) panicked at crates/agent-server/tests/http_image_input.rs:164:5:
上传等待落进 session actor，队列后的纯文本不该等 700ms
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test slow_upload_leaves_its_session_actor_available_until_the_reference_is_ready ... FAILED

failures:

failures:
    slow_upload_leaves_its_session_actor_available_until_the_reference_is_ready

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.74s

error: test failed, to rerun pass `-p agent-server --test http_image_input`
```
