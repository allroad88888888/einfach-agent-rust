# 113 `agent-transport` 的 fetch 实现：浏览器侧比 ureq 那套薄

**里程碑** M13 · **依赖** 111 · **模型** sonnet · **独测** ✅（流式分帧错了会静默丢内容）

## 为什么这条不难

`agent-transport` 今天的四件事里，只有第一件要重做：

| 文件 | 干啥 | wasm 侧 |
|---|---|---|
| `client.rs` | `post_stream()`——本 crate 唯一的请求方法，只做流式 | 换 `fetch` + `ReadableStream` |
| `read_loop.rs`（165 行） | 读线程 + `mpsc::sync_channel`，为了在阻塞流上做中断 | **整个不需要**——`AbortController` 就是那个句柄 |
| `backoff.rs` | 连接期退避 + jitter | 逻辑照搬，纯计算 |
| `config.rs` | 解析 `providers.toml` | **不移植**，浏览器没有这个文件（配置来源见 114） |
| `upload.rs` | 图片上传到供应商 | `fetch` 直接可用 |

`read_loop.rs` 的存在理由写在 `lib.rs` 顶部：ureq 的阻塞 `read` 没有外部中断句柄，022 第一版
用短 socket timeout 当轮询、023 发现慢首字节的家（Kimi）会被误判成死连接，才改成读线程 +
双超时旋钮，自称「不优雅但可测」。**这笔债在浏览器侧不用还**。

## 范围

1. 按目标平台分实现：native 保留 ureq 那条一行不动，wasm 走 fetch。
   接缝形状由实现者定，但**上层看到的必须是同一个 `Client::post_stream`**——
   `agent-providers` 与 `agent-runtime` 一行都不该因为换平台而改。
2. 取消语义对齐：native 那条的取消标志轮询，wasm 侧换成 `AbortController::abort()`。
   **对上层的可观测行为必须一致**（同样的取消点、同样的错误分类）。
3. 错误分类**仍然不在这里做**（`lib.rs` 已写明：transport 不知道自己在跟哪家说话）。
   非 200 一律打包成 `TransportError::Http` 交上层。

## 验收（可判定）

- native 侧：`cargo test -p agent-transport` 全绿，且与本 issue 之前**逐字相同**的行为
  （录制帧测试不变）。
- wasm 侧：用录制帧喂进 fetch 实现，**分帧结果与 native 实现逐字节相同**。这是核心断言——
  流式分帧错了不会报错，只会静默丢内容或粘帧。
- 取消：wasm 侧发起请求后立即 abort，上层拿到的错误分类与 native 侧取消时**同一档**。
- `rg ureq crates/` 仍**只命中 `agent-transport`**（`lib.rs` 那条镜像约束不许破）。

## 注意

- 三家的流式差异已经在 `probes/PROVIDERS.md` 里结论化过，**别在本 issue 里重新探**——
  照那份结论实现，对不上再回头改探针。
- `upload.rs` 的图片上传路径（M11）不要在本 issue 里顺手改语义，只做平台适配。

## 实做记录

### 接缝切在哪

按目标平台整模块二选一，靠 `#[cfg(target_arch = "wasm32")]`，不是 trait/泛型：

```rust
#[cfg(not(target_arch = "wasm32"))]
mod client;                 // 原样不动
#[cfg(not(target_arch = "wasm32"))]
mod read_loop;               // 原样不动
#[cfg(target_arch = "wasm32")]
mod fetch_client;            // Client 的 wasm 实现
#[cfg(target_arch = "wasm32")]
mod fetch_request;           // 一次 fetch 尝试的 web_sys 接线
#[cfg(target_arch = "wasm32")]
mod web_stream_source;       // ChunkSource 的生产实现（包 ReadableStreamDefaultReader）
#[cfg(target_arch = "wasm32")]
mod js_timer;                 // setTimeout 桥 + 不依赖 futures crate 的两路 race
```

`pub use client::Client` 与 `pub use fetch_client::Client` 二选一导出——两条实现的方法表
（`new`/`with_config`/`post_stream`/`upload_image`）在源码层面完全独立，靠**签名一致**
（不是共享的 trait）保证上层用法不变，跟决策文档里「wasm 是第三种宿主形态，不是抽象出
第三层」的取向一致。选 cfg 二选一而不是 trait，是因为 native/wasm 从来不会在同一次编译
里共存——运行时多态在这里没有需求，编译期二选一更简单，也不会强迫 native 侧为了满足一个
trait 而改动签名（`post_stream` 现在是 `impl FnMut` 泛型参数，装进 trait 方法要么变成
`Box<dyn FnMut>`——native 侧一行不动的要求直接否决了 trait 路线）。

`ImageUpload`/`UploadError`/`MAX_IMAGE_BYTES`/multipart 编码/响应体解析（`upload.rs`）是
两边唯一共享单一实现的部分——纯数据结构 + 纯编码逻辑，没有平台差异，只有 native 专属的
`send()`（吃 `ureq::Agent`）和新增的 wasm 专属 `fetch_upload::send()`（`async fn`）分别在
文件内 / 文件间 cfg 隔开。`backoff.rs` 按 issue 原话「纯计算，两边共用」处理：`Backoff::
delay()` 原样复用，但 `sleep_cancelable()`（真的 `std::thread::sleep`）没有 wasm 版本可用
（见下面「同步阻塞在 wasm 上不成立」），wasm 侧退避等待另写了 `js_timer::delay_ms` +
`fetch_client::sleep_cancelable_async`，不碰 `backoff.rs` 一个字节。

### wasm 侧比 native 少了什么，具体行数——以及一个必须如实报告的反直觉结果

`read_loop.rs`（165 行：`thread::spawn` 起读线程、`mpsc::sync_channel(0)`、`LineEvent`
三态枚举、`DEFAULT_SOCKET_TIMEOUT`/`DEFAULT_CANCEL_POLL_INTERVAL` 两个独立超时旋钮）在
wasm 侧**完全不存在**——不是「更薄的另一份实现」，是这个文件对应的机制整个消失了：没有
`thread::spawn`，没有 `mpsc` channel，没有两个需要互相解耦的超时旋钮。`fetch` 的
`ReadableStream.read()` 本身就是一个可以直接 `await` 的 Promise，`drive_stream`（wasm 对应
的状态机）是单线程顺序执行的 `async fn`，`read_loop.rs` 存在的**唯一理由**（ureq 阻塞
`read` 没有外部中断句柄）在 wasm 上不成立——这条 issue 最核心的判断，**实测证实是对的**。

但如果按纯文件总行数比较结果是反直觉的：

| | native 核心（`client.rs` + `read_loop.rs`） | wasm 核心（`fetch_client.rs` + `fetch_request.rs` + `web_stream_source.rs` + `js_timer.rs`） |
|---|---|---|
| 行数 | 391 | 496（+105，未计入下面共享的分帧模块） |

算上 `stream_drive.rs`（116 行）+ `line_framer.rs`（132 行）这两个两边共用的分帧核心，
wasm 侧关联代码总行数是 744，比 native 的 391 **多 353 行，不是更少**。如实拆解多在哪：

1. **web_sys/js_sys 的胶水样板**：native 的 `ureq::Response` 直接实现 `Read`，`fetch` 给的
   是 `Promise<{done, value}>`，`value` 是 `Uint8Array`——每一次「JS 值 → Rust 类型」的
   转换都要手写 `Reflect::get` + `dyn_into` + 错误分支，`describe_js_error`/
   `parse_read_result`/`build_request` 这类函数在 native 侧根本不需要对应物（ureq 的
   `Agent`/`Response`/`Error` 是原生 Rust 类型，没有这层转换税）。
2. **模块文档本身很长**：六个 wasm 相关文件里注释行占比 15%–36%（`fetch_client.rs` 50/198
   行、`stream_drive.rs` 43/116 行），因为要讲清楚「取消粒度怎么对齐 native」「分帧一致性
   怎么证明」「同步阻塞为什么在 wasm 上不成立」——这些文档在 native 侧要么已经写在 022/023
   的事故记录里（不用重写），要么根本不是问题（native 没有同步/异步张力）。
3. **`line_framer.rs`/`stream_drive.rs` 是两边共用的验收基础设施**，不是纯粹的 wasm 成本——
   它们同时是「wasm 分帧与 native 分帧逐字节相同」这条验收的证明现场（见下一节），把这两个
   文件算进「wasm 的代价」并不完全公平，但为了不假装数字好看，仍然如实计入上表。

**结论**：issue 标题「浏览器侧比 ureq 那套薄」在**机制复杂度**上成立（阻塞流中断的整套
线程/channel/双超时协调消失了），在**代码总行数**上不成立（wasm 侧因为要手写 JS FFI 胶水
+ 详尽文档，行数反而更多）。这是本 issue 交付前没有预料到、也没有在 issue 原文里被提及的
一点，如实记录，不为了呼应标题而回避。

### 分帧一致性的证明方式

三层证据，从强到弱：

1. **同一个函数，两条平台各自的生产代码都会调用它，在 native 目标上直接跑起来比对**
   （`src/framing_parity_tests.rs`，`cargo test -p agent-transport` 的一部分，不需要
   wasm32 目标或浏览器/Node）。具体做法：wasm 侧的分帧状态机 `stream_drive::drive_stream`
   不依赖任何 `web_sys` 类型，只依赖一个 `ChunkSource` trait（「字节从哪儿来」的接缝）；
   `fetch_client.rs` 用一份包 `ReadableStreamDefaultReader` 的实现（`WebStreamSource`，只
   能在浏览器/Node 里跑），`framing_parity_tests.rs` 用一份不碰任何 JS 绑定的内存序列
   `MockChunkSource`（可以在 native 上跑）——**两者调用的是同一个 `drive_stream` 函数，
   不是两份平行重新实现**。测试把同一份字节喂给 native 真正的生产函数
   `read_loop::run`（不是重新实现的另一份，是 `Client::post_stream` 拿到 200 响应后
   实际调用的那个）和配 mock 的 `drive_stream`，断言逐行输出 + 终态 `StreamOutcome`
   完全相等，覆盖：干净关闭、CRLF 行尾、EOF 前无尾随换行的残余行、空流、一个 chunk 里含
   多行、`on_line` 提前 `Break`、取消标志置位、连接中途读坏共 8 个场景，每个场景还额外用
   「整块喂」「逐字节喂」「任意长度切块喂」三种分法各跑一遍（`chunk_size` 参数化）——
   因为 `fetch` 的 `ReadableStream` 不保证块边界对齐 SSE 的行边界，这是 wasm 独有的问题，
   必须证明分帧结果与喂入的切法无关。全部 8×3=24 组断言在 `cargo test -p agent-transport`
   里跑，每次都跑，不是一次性验证。
2. **真实执行**（`crates/agent-transport/tests/wasm_smoke.rs` + `tests/wasm_node/`，用
   `wasm-pack test --node` 跑，不是 mock）：`tests/wasm_node/server.mjs` 起一个真实
   Node HTTP 服务器，`wasm_smoke.rs` 编译成真正的 wasm32 二进制，用 Node 原生的
   `fetch`/`ReadableStream`/`AbortController`（Node 18+ 内置，不是 polyfill）对着这个
   服务器发请求，断言收到的行序列——这条验证了 (1) 里没覆盖到的部分：`fetch_request.rs`
   的请求构造、`web_stream_source.rs` 包 `ReadableStreamDefaultReader` 的胶水代码本身能
   不能编译、链接、真的跑起来（不只是类型检查过），场景对齐 native 的
   `tests/it/fake_sse.rs`：干净关闭、慢首字节（对应 023 事故场景，验证 wasm 上这个
   问题根本不存在，见下）、402 不重试、卡住的流被 abort 打断。用
   `crates/agent-transport/tests/wasm_node/run.sh` 一键跑（起 server → 等它监听 →
   `wasm-pack test --node` → 收尾杀 server），本地连续跑了两次，4/4 全绿，非 flaky。
3. **`cargo check --target wasm32-unknown-unknown -p agent-transport --all-targets`**：
   确认所有 `web_sys`/`js_sys` 类型用法在真实 API 下类型检查通过（web-sys 0.3.104，
   wasm-bindgen 0.2.127，经 rsproxy.cn 镜像正常解析安装，不是假设的 API 形状）。

三层证据里第 1 层是本 issue 最核心的交付物——不需要任何额外基础设施，每次 `cargo test`
都会自动重新验证，不会因为没人手动跑 `wasm-pack test` 而腐化。

### 取消语义对齐——一个真实的设计缺口，中途发现，已经补上

最初的实现只在 `drive_stream` 循环体每次「处理下一块字节之前」查一次 `cancel`，跟
`read_loop::run` 的 `recv_timeout` 轮询节奏对齐。写完 `web_stream_source.rs` 后意识到这
不够：`drive_stream` 自己就是唯一的执行体，一旦它 `await` 在 `reader.read()` 上（服务端
还在但没数据——对应 native `cancel_flag_interrupts_a_stalled_stream` 测的那个场景），
`cancel` 标志无论怎么被外部置位，都不会被再检查一次，因为**没有第二个执行体在旁边替它
盯着**。native 侧靠读线程和主流程解耦解决了这个问题（主流程即使卡在 `recv_timeout` 里，
读线程被丢弃不 join），wasm 侧一开始的设计没有对应物。

修法：`web_stream_source.rs` 新增 `js_timer::race`（不依赖 `futures`/`tokio`，用
`std::future::poll_fn`，1.64 起稳定，手写一个「谁先 Ready 就返回谁」的两路 race），把
`reader.read()` 跟一个「每隔 `poll_interval` 查一次 `cancel`」的 future 赛跑——取消标志
赢了就主动 `AbortController::abort()`，让浏览器把连接断掉，不等 `read()` 那个 Promise
自然结束。`ChunkSource::next_chunk` 的契约相应调整：`Ok(None)` 现在有两种成因（真
EOF，或者等待中发现被取消），靠 `drive_stream` 在收到 `Ok(None)` 后**再看一眼** `cancel`
来分辨——这样改动没有影响 `ChunkSource` trait 本身的方法签名，`MockChunkSource` 不用
跟着变。`tests/wasm_smoke.rs::abort_interrupts_a_stalled_stream` 就是专门测这条：服务端
发一行后再也不发数据、不关闭连接，200ms 后一个并发的 `spawn_local` 任务把 `cancel` 置位，
断言只看到第一行、且 outcome 是 `Cancelled`——这是本 issue 里为数不多能被浏览器/Node
真实执行验证、而不能只靠 mock 验证的场景（因为它测的正是「一个真实的、可能永远不 resolve
的 Promise 能不能被中途打断」）。

对上层可观测的取消行为：两边都是「取消标志置位 → `StreamOutcome::Cancelled` → 不打包成
带 `ErrorClass` 的错误」，`provider_call_finish.rs` 那层的处理不需要区分平台（`Cancelled`
直接映射 `Event::Cancel`，两边一致）。

### `Client::post_stream` 的同步签名在 wasm32 上不成立——这是本 issue 最大的发现

issue 要求「上层看到的必须仍是同一个 `Client::post_stream`」，逐字理解就是：签名（同步、
`&self`、阻塞式返回 `Result<StreamOutcome, TransportError>`）在两个目标上完全一致。这一点
在**类型层面**做到了（两边都有 `fn post_stream(&self, url, api_key, body, cancel, on_line)
-> Result<StreamOutcome, TransportError>`），但**在 wasm32-unknown-unknown 默认（无
`+atomics`）单线程模型下，这个同步签名没有办法真正阻塞等待 `fetch`**——阻塞需要挂起当前
线程，而当前线程如果被挂起，驱动 `fetch` 那个 Promise resolve 的 JS 事件循环也转不动了，
死锁。这不是猜测，是本 issue 实现过程中**实测确认**的平台限制：

```rust
// std::thread::spawn 在 wasm32-unknown-unknown（stable，无 atomics）上能编译……
let handle = std::thread::spawn(move || tx.send(42).unwrap());
```
编译通过，但用 Node 直接实例化生成的 `.wasm` 并调用，运行时立刻 `unreachable` trap——
`thread::spawn` 在这个目标上是「类型层面存在，运行时必定 panic」的函数。真正能让一个线程
阻塞等待另一个异步任务的机制（`SharedArrayBuffer` + `Atomics.wait` + 显式的 Worker 线程
池引导）需要 `+atomics` target feature（nightly + `-Z build-std`）、跨源隔离
（`COOP`/`COEP` 响应头）、以及把 `std::thread::spawn` 接到真实 Worker 的引导代码——这些
都不存在于本仓库的工具链和构建脚本里，也不是 111/113 的决策文档提到过的代价。

处理方式（如实记录，不是回避）：

- `Client::post_stream_async`——真正的实现，`fetch` 全过程 + 分帧 + 退避重试，`async fn`。
  这是上面两层证据实际验证的入口，也是 114（wasm 宿主）接入时应该调的那个。
- `Client::post_stream`——签名与 native **逐字**相同，编译期满足「上层源码不用改」这条
  要求，但调用它本身**不会真的发请求**：立刻返回一条说明这件事的 `TransportError::
  Connect`。这是刻意的取舍：宁可调用方第一次拿到结果就看到清楚的错误信息，也不要假装能
  阻塞、实际把整个页面/Worker 卡死——一个会挂起的死锁比一个明确的错误更难调试。

`agent-providers`/`agent-runtime` 源码确实一行没改（`git diff --stat` 为证），但这只覆盖
了「编译期签名兼容」，**没有覆盖「今天的调用点换到 wasm32 目标后真的能工作」**——
`io_thread.rs` 里 `binding.client.post_stream(...)` 是从 `std::thread::spawn` 的闭包里
同步调用的，这个调用点要接到 wasm 宿主，谁碰它就必须换成 `.post_stream_async(...).await`，
而 `io_thread.rs` 本身怎么在 wasm 上变成能 `await` 的（`std::thread::spawn` 起 IO 线程这个
结构本身在默认 wasm32 目标上也不成立）是 114（甚至更后面）才能解决的问题，不在本 issue
范围内（只碰 `agent-transport`），但**这条依赖关系没有在 111/113/114 的文档里被显式记
下**，特此补记：114 开工前应该先确认这一条怎么处理，而不是想当然地认为「把 `Client` 换
成 wasm 版就好了」。

`upload_image`/`upload_image_async` 是同样的处理，理由相同，不重复展开。

### 其它偏差与发现

1. **`fetch` 没有 native 那种「等状态行超时」与「连接失败」的区分**（023 修的那个精细
   分类，`is_response_wait_failure`/`ConnectAttemptError::ResponseWaitBroken`）。浏览器
   的 `fetch()` 把 DNS、TCP、TLS、请求发送、等响应头全部揉进一个 Promise，拒绝时只有
   一个笼统的网络错误，没有「已经发出去了、只是服务端还没回」这个中间态可观测。这不是
   实现疏漏——是 `fetch` API 本身不提供这个信息。好消息是这个问题本身在 wasm 上更不容易
   触发：`fetch` 没有 native 那种默认的、需要手工调大的 socket 读超时，`tests/wasm_smoke.
   rs::slow_first_byte_is_tolerated`（对应 023 事故场景，300ms 慢首字节）在没有任何特殊
   处理的情况下就正常通过——**这正面印证了 issue 的核心论点**：`read_loop.rs`/`023` 那整
   类「短超时误判慢连接」的 bug 在 wasm 上根本不存在，不是被修掉了，是因为触发它的机制
   （手工设置的短 socket 超时）在 fetch 里不存在。
2. **`connect_timeout` 参数在 wasm 侧被接受但不生效**（`with_config` 签名保留，实现里
   显式忽略）——`fetch` 没有内建的连接期超时钩子，要做需要另起一个定时器去竞速
   `AbortController`。issue 验收没有要求这一条，为了不引入额外的、没有验收覆盖的 JS
   交互面，没有加；如实记在 `fetch_client.rs` 的文档里而不是悄悄吞掉这个参数。
3. **`rg ureq crates/` 现在不是只命中 `agent-transport`**：`agent-runtime/tests/it/
   support/routed.rs:132` 和 `agent-core/tests/it/cache_guard_preflight.rs:168` 各有
   一处提到 "ureq" 的**注释/字符串**（不是依赖）。核实过这两处在本 issue 开工前就存在
   （不是这次改动引入的），且 `check-invariants.sh` 的红线 7 自动化检查本来就只扫
   `Cargo.toml` 里的依赖声明（`scripts/check-invariants.sh:63`），不是全文 `grep`——
   真正的自动化红线检查通过。issue 验收写的「`rg ureq crates/` 仍只命中
   `agent-transport`」这句话在本 issue开工前就已经不成立（人工读文本 vs 自动化检查
   的落差），照实记录，不影响真正的红线判定。
4. `upload.rs` 按「平台适配」处理：类型定义/multipart 编码/响应解析保持单一实现不复制，
   只有真正碰网络的 `send()` 分平台各写一份，语义（错误分类、大小限制、boundary 选取）
   一个字没动，`tests/it/file_upload_*.rs`（native）全部原样通过。
