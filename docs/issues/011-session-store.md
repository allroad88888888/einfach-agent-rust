# 011 `SessionStore` 端口与两个实现

**里程碑** M2 · **依赖** 010 · **模型** sonnet · **独立测试 agent** 否 · **状态** 完成

## 目标

持久化端口 + `Memory`（测试）与 `Jsonl`（文件追加）两个实现。

## 接口

```rust
trait SessionStore {
    fn append(&self, id: SessionId, entry: &Entry);
    fn drop_oldest(&self, id: SessionId, count: usize);   // cap 溢出
    fn drop_after(&self, id: SessionId, cursor: usize);   // 新分支覆盖 redo 尾
    fn set_cursor(&self, id: SessionId, cursor: usize);
    fn snapshot(&self, id: SessionId, snap: &Snapshot);
    fn load(&self, id: SessionId) -> Option<(Snapshot, Vec<Entry>, usize)>;
}
```

## 两个刻意的设计

**写入全部 fire-and-forget，没有返回值。** 失败不回滚内存状态，只经 `on_error` 上报
——否则一次 IO 抖动就会让 undo 永久卡死。这是上游 TS 版踩过的坑，直接采纳结论。

**同步 trait。** actor 是单线程的，写入走 mpsc 扔给专门的 IO 线程，actor 不阻塞，
`agent-core` 也不用染上 async。

## 验收

- `Memory` 与 `Jsonl` 都能完成「写入 → 进程重启 → 载入 → 恢复」
- IO 失败时 undo 仍然可用（内存状态不受影响），且 `on_error` 收到通知
- 可 per-session 选后端（临时会话 `Memory`，重要会话落盘）

## 注意

`Jsonl` 的实现不进 `agent-core`（红线 7：不得做 IO）。端口定义在 core，
实现放 `agent-store` 或单独的 crate。

## 实做记录

### 与原文的三处修正（主会话已在派工时定，这里记落地形态）

1. **trait 不带 `SessionId`**：一个实例绑一个会话，构造时给身份/路径。M3 多会话是
   「每会话一个实例」。原文的 `id: SessionId` 参数全部删掉。
2. **端口与 `Memory` 在 `crates/agent-store/src/persist/`（`mod.rs` + `log.rs` +
   `memory.rs`）；`Jsonl` 在 `crates/agent-runtime/src/jsonl/`**（`mod.rs` +
   `io_thread.rs` + `load.rs` + `record.rs` + `error.rs`）。`agent-runtime`
   的 `Cargo.toml` 新增 `agent-store` 依赖（此前没有——`Jsonl` 要用
   `Entry`/`Snapshot`/`SessionLog` 三个类型）。
3. **泛型对齐 history**：`trait SessionStore<K, V, M>`，`LoadedSession<K, V, M>`
   ——跟 `History<K, V, M>` 同一组参数，不是原文那个不带类型参数的裸 trait。

### 钉死的端口，字面落地

```rust
pub trait SessionStore<K, V, M> {
    fn append(&self, entry: &Entry<K, V, M>);
    fn drop_oldest(&self, count: usize);
    fn drop_after(&self, first_seq: u64, count: usize);
    fn set_cursor(&self, cursor: usize);
    fn snapshot(&self, snap: &Snapshot<K, V>);
    fn load(&self) -> Option<LoadedSession<K, V, M>>;
}
pub struct LoadedSession<K, V, M> {
    pub snapshot: Option<Snapshot<K, V>>,
    pub entries: Vec<Entry<K, V, M>>,
    pub cursor: usize,
    pub next_seq: u64,
}
```

`set_cursor` 收 `History::cursor()` 的**原样值**（相对 `History` 自己当前那份
`entries`，已经把 `enforce_cap` 的缩短算在内），调用方不用做任何换算——换算全部是
实现自己的事。`drop_oldest`/`drop_after` 直接转发 `History::take_drop_events()` 产出的
`DropEvent::Oldest{count}` / `DropEvent::RedoTail{first_seq, count}`，字段名对字段名。

### `SessionLog`：两个后端共用的记账引擎（`agent-store/src/persist/log.rs`）

011 要求「`Memory` 与 `Jsonl` 都过同一套端口行为测试（写→load→重放语义一致）」——
两个后端如果各自独立推一遍「游标要不要换算、换算成什么」，迟早在某个边界情况上分岔，
而且是「测试各自都过、行为却不一致」的那种分岔。于是把这段推导抽成一个零 IO 的引擎
（`SessionLog<K, V, M>`，`agent_store::persist::SessionLog`，`pub`），`Memory` 用
`Mutex<SessionLog<..>>` 直接包一层；`Jsonl` 的 IO 线程养一份同样的引擎决定该往文件里
写什么，`load()` 重放时再喂给一份全新的 `SessionLog` 决定最终状态——写路径和读路径
共用同一份「状态该怎么变」的代码，不是两套独立实现。

核心不变量：`held`（`SessionLog` 手里还留着的 entries）在逻辑上等于
`History.entries()[boundary..]`——`boundary` 是一个 `usize`，随两条独立路径移动：

- `record_snapshot`：`held` 里现在这些全部被这张快照代表了，`boundary += held.len()`，
  `held` 清空。
- `record_drop_oldest(count)`：`History` 自己的 `entries` 缩短了 `count`（cap 驱逐），
  `boundary` 跟着退——`count <= boundary` 时被切的本来就在 `held` 之前（早被快照吃过），
  `held` 不用动；`count > boundary` 时多出来的 `count - boundary` 条要从 `held` 前端
  真删。这条推导踩过一次坑（见下「找到的一个真 bug」），有专门的单元测试
  （`agent-store/tests/session_log_replay.rs`）和一个驱动真实 `History` + 真实
  `store::Store` 的全链路测试（`agent-store/tests/session_store_memory_full_chain.rs`）
  钉住。

`to_loaded()` 返回的 `cursor`/`next_seq` 已经满足 `History::from_parts` 的三条不变量
（`cursor <= entries.len()`、`next_seq` 大于最后一条的 `seq`），调用方不用另外校验。

**已知的精度损失**：如果崩溃发生在「快照之后又 undo 回快照点之前」——`History` 自己完全
有能力 undo 到那么远——持久化这一侧没有能力精确表达「回到已经被压实掉的那一步」，
`last_cursor < boundary` 时钳到 0（相当于「把 `held` 里的都退干净」），是能给出的最接近
的答案，不是 bug。推给 027：如果 undo 粒度是 turn 且快照按「每 N turn 一张」的策略落，
这个退化场景只有「重启前一步 undo 越过了上一张快照」才会碰到，属于小概率但要在文档里
提一句，别让人以为持久化的 undo 精度和内存里的完全等价。

### 找到的一个真 bug：压实截断文件之后不能落「原始值」

设计阶段以为 `set_cursor`/`drop_oldest` 直接把调用方给的原始值序列化进文件就够了，写
`session_store_backend_choice.rs` 时炸了：`Memory`（单一份连续存活的 `SessionLog`，
`boundary` 从进程开始一路累积）和 `Jsonl`（`load()` 用一份**全新**的 `SessionLog`
重放，`boundary` 从 0 起步）对同一段调用方代码给出了不同的 `cursor`。

根因：快照落盘会把文件**截断**（旧 entries 压实掉），只留最近这一张快照往后的内容。
重放端看不到「压实之前」发生过什么，它的 `boundary` 天然从 0 起步；而 `SetCursor`/
`DropOldest` 收到的原始值是相对 `Jsonl` 内部那份**连续、真实、累积**的 `boundary`
定义的。两个 `boundary` 不是同一个数，原样落盘就是两套坐标系硬凑在一起，静默算错
——不 panic、不报错，正是 `docs/INVARIANTS.md` 说的那种最贵的 bug。

修法（`SessionLog` 新增两个只读入口，`agent-runtime/src/jsonl/io_thread.rs` 落盘前调）：

- `SessionLog::relative_cursor(&self) -> usize`：`to_loaded()` 里那个换算单拎出来，
  `SetCursor` 落盘时用它算出来的值，不用调用方给的原始 `cursor`。
- `record_drop_oldest` 从 `fn(&mut self, count: usize)` 改成返回
  `fn(&mut self, count: usize) -> usize`——「这一次真正从 `held` 前端切掉了多少条」。
  `DropOldest` 落盘时写这个返回值，不写原始 `count`。

两者的公共道理：落盘的必须是「已经相对当前 `boundary` 算过净效果」的量，这样的量在
`boundary = 0` 的世界里重放，效果和原始事件在真实 `boundary` 的世界里发生的完全一致。
`DropAfter` 不用这样处理——它按 `seq` 谓词过滤尾部，`seq` 在两侧指的是同一批物理条目，
没有坐标要换算。

专门盯这条回归的测试：`agent-runtime/tests/session_store_jsonl_cap_crosses_snapshot.rs`
（cap 驱逐横跨快照压实边界 + 真实「进程重启」）。

### `Jsonl` 格式

Append-only 行式文件，每行一个 `serde(tag = "kind")` 内部标签的 JSON 对象
（`agent-runtime/src/jsonl/record.rs`），五个变体对应 `SessionStore` 的五个写方法：

```
{"kind":"entry","seq":0,"meta":...,"changes":[{"key":...,"prev":...,"next":...}]}
{"kind":"snapshot","values":[[key,value],...]}
{"kind":"cursor","cursor":2}
{"kind":"drop_oldest","count":1}
{"kind":"drop_after","first_seq":3,"count":2}
```

`cursor`/`drop_oldest.count` 落的是「换算过的净效果」，不是调用方给的原始值——见上面
那条 bug 记录。`drop_after` 的 `first_seq`/`count` 原样落盘。

写全部发生在构造时起的一个专用 IO 线程上（`std::thread::spawn` + `mpsc::channel`，
无界、`send` 从不阻塞），`SessionStore` 的五个写方法只是把消息塞进 channel——actor
从不等磁盘。`flush()`（公开的额外方法，不在 trait 上）发一条 `Msg::Flush(oneshot)`
进队列排在最后，收到 ack 即代表前面所有写入真的处理完了（落盘或者确认放弃）；
`load()` 内部先 `flush()` 再直接在调用线程上读文件——一份刚构造、代表「进程重启」的
`Jsonl` 没有任何活体镜像，只能从文件本身重建，不经过 IO 线程。`Drop` 先关发送端
（channel 关闭 IO 线程的 `recv()` 循环才会退出）再 `join`，这就是「drop 时排干」。

### 压实策略

`snapshot()` 到达时 IO 线程内的 `SessionLog` 镜像已经把 `held` 清空——这一刻文件里
「快照之前的旧 entries」全部过时。用 `File::set_len(0)` 截断（`File` 是 append 模式
打开，截断之后下一次 `write_all` 仍然从新的文件尾写起，不需要额外 `seek`）再只写这
一行 `Snapshot` 记录；之后新的 `Entry`/`Cursor`/`DropOldest`/`DropAfter` 正常追加在
它后面。`load()` 重放时见到一个 `Snapshot` 记录就重置累积器，文件里因此**最多同时
存在一张快照**（整份重写而不是分段——会话粒度的日志量级不需要分段，重写的开销是
O(压实之后剩下的行数)，不是 O(全部历史)）。

### 崩溃语义（`load.rs`）

先整份读入内存按行切，逐行 `serde_json::from_str::<Record<..>>`：

- **非最后一行**解析失败 → 中部损坏：经 `on_error` 报 `CorruptLine{line}`，整份
  `load()` 返回 `None`——不静默丢中段、不加载半份状态。
- **最后一行**解析失败 → 尾部半行（append-only 写到一半断电/被杀的诚实语义）：
  经 `on_error` 报 `TruncatedTail{line}`，从这一行截断，前面的内容照常加载。

`SessionStoreError` 只带「哪一行、什么类别」，不转发 `serde_json::Error` 的 `Display`
（那玩意有时会把解析到一半的值片段带出来）——绝不把 K/V 内容带进日志/错误，
`session_store_jsonl_corrupt_files.rs::the_error_never_carries_the_offending_lines_content`
专门测了这条。

### IO 失败

`Jsonl::new` **从不失败**——构造不返回 `Result`，即便 `path` 当场打不开（只读目录、
父目录不存在）。失败经构造时传入的 `on_error` 报**一次**（IO 线程刚起来、第一次
`OpenOptions::open` 那一刻），之后每条写消息静默吞掉，不重复报告——根因只有一个，
报一百次和报一次传达的信息量相同。任何一次写失败（包括打开成功之后中途失败，比如
磁盘写满）同样报一次并把内部的 `file` 记成 `None`，之后的写入静默吞掉。全程不 panic；
调用方自己的 `History`/`Store` 完全不受影响——`SessionStore` 从不回读，写入语句本身
不会因为持久化失败而失败。

### 验收逐条对应

- 「Memory 与 Jsonl 都过同一套端口行为测试」：`agent-store/tests/session_log_replay.rs`
  + `session_store_memory_full_chain.rs`（`Memory`，含真实 `History`/`Store` 全链路 +
  cap 驱逐横跨快照边界）；`agent-runtime/tests/session_store_jsonl_roundtrip.rs`（同一段
  调用方代码的 `Jsonl` 版本）。
- 「写入 → 进程重启 → 载入 → 恢复」：`agent-runtime/tests/session_store_jsonl_crash_recovery.rs`
  （真临时文件，`Jsonl` 实例整个 drop 再新建，配合 `from_parts` + `apply_next` +
  `apply_prev` 全链路，手法照抄 `snapshot_recovery_is_redo.rs`）；
  `session_store_jsonl_cap_crosses_snapshot.rs`（同上，额外叠 cap 驱逐横跨快照边界）。
- 「IO 失败时 append 不 panic、on_error 收到、内存侧调用方一切照旧」：
  `session_store_jsonl_io_failure.rs`（父目录不存在——比只读目录权限位更环境无关，
  部分沙箱/CI 以 root 跑测试时权限位不生效）。
- 「尾部半行/中部损坏两种坏文件各一测」：`session_store_jsonl_corrupt_files.rs`
  三个测试（两种坏文件 + 一个「错误不带内容」的红线测试）。
- 「per-session 选后端」：`session_store_backend_choice.rs`（一个泛型
  `drive_session::<S: SessionStore<..>>` 分别喂 `Memory`/`Jsonl`，另加一个
  `Vec<Box<dyn SessionStore<..>>>` 证明两个后端能同时装进同一个宿主容器）。

### 命令输出

```
cargo test -p agent-store -p agent-runtime   # 全绿，见下方汇总
cargo test --workspace                        # 130 个测试二进制全部 ok，0 failed
cargo clippy --workspace --all-targets -- -D warnings   # 0 警告
bash scripts/check-invariants.sh --all        # 红线检查通过
```

`agent-store`：66（unit）+ 应用/undo/snapshot/session_log/session_store 系列共 30 个
集成测试文件，全部 `ok`（新增 `persist` 相关：`persist/log.rs` 2 个内联单测、
`persist/memory.rs` 3 个内联单测、`tests/session_log_replay.rs` 7 个、
`tests/session_store_memory_full_chain.rs` 2 个）。

`agent-runtime`：5（unit）+ 9 个集成测试文件全部 `ok`（新增 6 个 `session_store_*`
文件，共 10 个测试）。

行数：`persist/mod.rs` 78、`persist/log.rs` 219、`persist/memory.rs` 116；
`jsonl/mod.rs` 154、`jsonl/io_thread.rs` 153、`jsonl/load.rs` 78、`jsonl/record.rs` 23、
`jsonl/error.rs` 35。全部 ≤300（`persist/log.rs` 是唯一超过 150 行的，仍在软上限内，
没有触发拆分提示）。

### 异议 / 推给 027 的事

- 没有异议——三处修正在派工时已经定了，钂死的接口原样落地，没有需要主会话裁决的
  接缝问题。
- **推给 027**：`from_parts` 出来的 `History` 无 cap（`persist.rs` 的 `boundary` 逻辑
  跟 010 的 `to_parts`/`from_parts` 一样不管 cap），会话层载入之后要自己
  `set_cap(Some(100))`（或配置值），漏了就是「重启之后日志不再受限」——跟 010 实做
  记录推给别人的那条同一件事，这里再钉一遍因为 011 是真正会被 027 调用的那一层。
- **推给 027**：`SessionStore` 的调用顺序有一条隐含契约——一次 command 之后先
  `append`，再 `set_cursor`；`take_drop_events()` 转发 `drop_oldest`/`drop_after` 放在
  `append`+`set_cursor` **之后**（`agent-store` 侧所有全链路测试的 `command` 帮助函数
  都是这个顺序）。这不是随意的：`drop_oldest`/`drop_after` 依赖 `SessionLog` 当前的
  `boundary`/`held` 状态是「这一步已经生效」之后的状态，调换顺序不会 panic，但可能让
  `SessionLog` 的推导基于错误的中间态——没有专门测过反过来的顺序，027 接线时按
  `command` 帮助函数那个顺序写就对。
- **推给 027**：「快照之后 undo 回快照点之前，重启后精度丢失」的已知限制（见上）——
  M2 若真的出现「/undo 越过很多轮直接到 5 轮前的某个快照区间」这种操作，写文档告诉
  用户「太旧的 undo 在重启后可能不精确」，不是这个端口能力范围内的事。

### 合并记录（主会话）

三处修正按令落地（无 SessionId / 端口+Memory 在 store、Jsonl 在 runtime / 泛型对齐）。
亮点：共享 SessionLog 记账引擎让「压实后 cursor 换算」只写一次；实现中自抓自修了
Memory/Jsonl 对快照点后 cursor 理解不一致的静默错值 bug（换算净效果落盘 + 回归测试）。
坏文件两态、IO 失败不 panic、错误不带 K/V 内容各有专测。留给 027 的三条在实做记录
（载入后重调 set_cap、DropEvent 转发的调用顺序契约、跨压实边界 undo 的恢复精度限度）。

### 契约更正（027 实做时发现，2026-08-02）

上文「append + set_cursor 先于 take_drop_events 转发」的调用顺序**对 RedoTail 是错的**：
record_drop_after 按绝对阈值 retain(seq < first_seq)，新条目的 seq 必然更大，
先 append 再转发会把它一并冲掉——真丢数据。正确契约：**RedoTail 在 append 新条目
之前转发，Oldest 在之后**（与 History 内部 enforce_cap 的时序一致）。两类事件
对顺序要求相反，必须拆开。回归测试在 agent-runtime persist/sync 与
agent-store session_log_replay 两侧。

### 契约更正（027 独立测试 agent 发现，2026-08-02）：`load()` 三态化

`SessionStore::load(&self) -> Option<LoadedSession<K, V, M>>` 的 `Option` 把「这个
身份从来没写过东西」和「有会话但 `Jsonl::load()` 自己因为中部损坏拒绝加载」压缩
成了同一个 `None`——本文档「崩溃语义」一节早就设计了中部损坏要整份拒绝，但拒绝
之后的返回值跟"从没写过"用了同一个信号，宿主（`agent-cli::main`）拿到 `None` 只
能当成"开新会话"，警告打了，下一张快照就把用户原本还能人工修复的损坏文件覆盖了
——独测 027 时抓到的真 bug，不是这次新引入的行为，是端口签名从一开始就没给宿主
留下"区分"的手段。

修法：`load()` 签名改成 `-> LoadOutcome<K, V, M>`（`agent-store/src/persist/mod.rs`
新增的三态 `enum`：`Absent` / `Refused{reason}` / `Loaded`），`Memory`/`Jsonl` 两个
实现与所有调用方同步改——**这是许可的 trait 变更**，`Memory` 永远不会给出
`Refused`（零 IO，没有序列化步骤，参见其新增单测
`memory_never_refuses_a_load`）；`Jsonl::load()` 的 `CorruptLine`/尾部 IO 错误分支
翻成 `Refused{reason}`（`reason` 复用既有 `SessionStoreError::Display`，同样不带
K/V 内容），`TruncatedTail` 容忍语义不变（仍是 `Loaded` + warn）。

`agent_runtime::persist::recover` 把 `LoadOutcome::Refused` 翻成新增的
`RecoverError::Refused(String)`，跟既有的 `UnknownLabel`/`InvalidHistory` 走同一个
`Err` 出口——`main.rs` 原有的 `Err(e) => fail(...)` 分支不用改一行就自动接住三态化
之后的新失败路径，硬失败、不带对话内容、原文件一字不动。回归测试与详细记录见
`docs/issues/027-cli-undo.md` 的对应更正条目（`agent-cli/tests/indep_corrupt_session.rs`
的中部损坏测试按新语义改了断言，注明来由）。

### 契约更正（027 独立测试 agent 发现，2026-08-02）：`Jsonl` IO 线程的 `mirror` 必须追平已有文件

`agent-runtime/src/jsonl/io_thread.rs::run()` 起步时 `mirror: SessionLog<K,V,M>` 恒为
`SessionLog::new()`——本文档「压实之后为什么不能落原始值」一节的推导全部建立在
「`mirror` 从进程一开始就连续参与了这份文件的完整历史」这个前提上，但「重启」恰恰
打破它：一份全新 `Jsonl` 实例、一条全新 IO 线程，如果文件里已经有上一个进程写的、
未经任何快照压实的内容，这份新镜像对那些内容一无所知——`held` 从空开始，
`SetCursor` 落盘的 `relative_cursor()`（`cursor.min(held.len())`）因此被系统性地
算小。下一次重启，`recover()` 读到一个 `cursor < entries.len()` 的会话（明明什么
都没 undo 过），它自己的下一次写入被 `History` 当成「覆盖 redo 尾」处理，上一个
周期真实写过的整轮对话被一条 `drop_after` 悄悄冲掉——不 panic、不报错，是红线
1-6 点名的那类最贵的静默错值 bug，只是这次出现在 cursor 通道。两个周期的重启测试
（`jsonl_restart_continues.rs`）测不到这条：它只重启一次、写一轮就结束，没有第三次
读盘去验证第二轮的数据有没有被冲掉；独测 027 时写「连续三个周期」回归测试才现形。

修法：`agent-runtime/src/jsonl/load.rs` 拆出 `seed_from_disk(path) -> SessionLog<K,V,M>`
（跟 `load()` 共用同一条重放逻辑 `replay()`，唯一差别是静默——不经 `on_error`，真正
的错误报告属于应用层显式调用 `load()`/`recover()` 的那一次）；`io_thread::run()`
起步调它，把 `mirror` 追平到文件已有内容再进入消息循环。全新会话/文件读不出来/
中部损坏都退化成一份空日志，跟以前的行为完全一致，不影响那些场景。回归测试
`agent-runtime/tests/jsonl_three_restart_cycles_keep_seq_increasing.rs`（三个周期，
断言第三次恢复的 `messages().len()` 一条不少）。