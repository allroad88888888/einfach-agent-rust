# 157 wasm 宿主同路：页面声明开局块（后置，等在飞工作落地）

**里程碑** M17 · **依赖** [155](155-with-host-prefix.md) + [156](156-server-prefix-declaration.md) + **另一会话的 agent-wasm capabilities 工作合并** · **模型** sonnet · **独测** — · **状态** **后置（2026-08-12）——不阻塞 M17 收口**

## 为什么后置

开工勘查（2026-08-12）发现：**HEAD 上的 `agent-wasm` 还没有任何 capabilities
声明路**——`capabilities.rs`、`config.rs` 的 `with_declared_capabilities`、
`assemble.rs` 的 `record_capabilities`/`run_session_start` 接线，全部是另一会话
**未提交的在飞工作**（主检出的脏区）。在它落地前做本条 = 在未合并的地基上盖楼，
且必然冲突。**M17 以 158（server 形态真机）收口，本条等地基合并后作为补做**。

## 目标（地基落地后生效）

浏览器宿主（页面声明）同一条路：页面 capabilities 加 `prefix`，建时落店、
恢复读回、装配尾接 `with_host_prefix`（155）。若届时 wasm 已接
`run_session_start`，合成条目自动被它跑到，驱动零改动。

## 做什么

1. wasm 侧 capabilities 解析加 `prefix` 字段，校验规则**逐条对齐 156 的
   validate**（前缀、内部重名、与 tools 重名、空 text）——两边规则出入即 bug，
   测试逐条镜像。
2. 建时 `declare_host_prefix`（154），恢复时 `host_prefix()` 读回，装配链尾接
   `with_host_prefix`。
3. `scripts/build-wasm.sh --dev` 过。

## 验收

- 声明一块 → 新建会话首轮料的 system 段含 `init:<name>` 块且在内置块之后。
- 恢复路：不重跑、值从店回放。
- 不声明的页面：装配产物与本条落地前逐字节相同。
- 校验拒绝路：坏前缀/空 text 当场拒，错误文本点名。

## 注意

- 开工前先确认另一会话的 agent-wasm 工作**已合并**且 `git status` 无交集脏区；
  仍有在飞就继续等，不要抢改。
- 本条的落点符号（模块名、函数名）以**届时合并后的真实代码**为准，
  上面写的名字来自在飞工作的一瞥，可能会变——先 `ls` + `grep` 再动手
  （WORKFLOW §四第 0 步）。
