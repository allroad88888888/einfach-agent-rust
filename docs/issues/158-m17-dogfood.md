# 158 真机收官 + 文档清账 ← M17 终点

**里程碑** M17 · **依赖** [156](156-server-prefix-declaration.md)（157 当时后置，不阻塞本条；它已于 08-13 补做完成） · **模型** 主会话前台 · **独测** 本条即验收 · **状态** 完成（见文末，2026-08-12）

## 目标

真 provider（DeepSeek，providers.toml）走一遍宿主声明开局块的完整生命周期，
然后把决策 31 的账清干净——M17 收口。

## 真机脚本（HTTP server 形态）

1. `POST /sessions` 带 `capabilities.prefix`（一块业务上下文文本）→ 首轮真实
   请求体里块在 system 段、在内置块之后。
2. 跑三轮对话，第 2 轮起 `cached/prompt ≥ 0.9`（声明块是前缀的一部分，
   不该破缓存）。
3. `kill -9` → 重开 → 再跑一轮：请求体前缀与崩溃前逐字节一致（sha256 比对），
   缓存命中率不掉。
4. spawn 子 agent：`inherit_prefix: []` 的子看不到声明块、缺省的子看得到
   （两份子请求体比对）。
5. 有历史后再带声明 → 400 `session_has_history`（真机路径复验）。
6. 不带声明的对照会话：请求体与 M17 之前的二进制 sha256 相等。

## 文档清账

- ROADMAP §四：「宿主声明不了 timed 工具」条目画勾指向决策 31（已在开工提交
  里写好，这里核对）；§二现状补 M17 一段。
- issues/README.md：M17 进度回填。
- CLAUDE.md 当前状态：M17 完成。
- HOST-CAPABILITIES.md §八之三 与实做对齐（如实修偏差）。

## 注意

- providers.toml 是 gitignored 的真钱 key，**绝不入库**；真机探针单飞
  （WORKFLOW 的既有纪律）。
- 每条验收都要留数字（命中率、sha256、HTTP 码），回填进本文件实做记录。

## 实做记录（主会话前台，2026-08-12，server 形态 + 真 DeepSeek）

**六条全过 + 两个白捡的发现。** provider = deepseek/deepseek-v4-flash。

### 线级（本地 recorder 当 provider，149 同款）

1. **声明块在真实请求体里的位置**（1 skill + 2 乱序 prefix 块）：system 段
   `ops-manual` 索引行(offset 53) < `A 段简报`(86) < `Z 段备注`(104)——
   内置索引块在前、声明块按 name 序在后；skill 正文不在 system（懒加载完好）。
2. **跨二进制 sha256 相等**：不声明的同一句输入，本次改动后的 server 与
   `bb43c83`（154 之前）现编的老 server，body sha256 同为 `60b5d254…de90`
   （7294 字节）。老二进制验真：`strings` 里 `capabilities.prefix` 0 次 vs 新 4 次。

### 真机（口令实验：简报块藏 `HUANGHE-6621-BEACON`）

3. **模型看得见声明块**：首轮自我介绍主动引用简报内容；第二轮零工具直接答出
   口令原文。
4. **缓存**：跳 2 = 2304/2369 (97.3%)、跳 3 = 2304/2397 (96.1%)、恢复后
   = 2304/2426 (95.0%)，全部 ≥ 0.9（跳 1 冷启 0% 属预期）。
5. **`kill -9` 恢复**：`outcome=recovered`；同一句口令原样答出；journal 里
   `prefix_init` 恰 1 条、sha `cd748309b62cdd8e` 崩溃前后一致（不重跑）。
6. **spawn 活对照**（决策 28 × 31 的交点）：`inherit_prefix: []` 的子缓存在
   128 token 处断（块不在它 system 里的旁证）、真心答「不知道」；缺省继承的子
   prompt 恰多 43 token（≈简报块体量），**思考原文第一句就是
   "From the briefing: HUANGHE-6621-BEACON"**——块在、它知道，只是出于
   「疑似套口令」的安全直觉对外答了不知道。行为对照成立，且是比预期更强的证据
   （思考里逐字引用）。
7. **dormant + 再声明 → 400 `session_has_history`**，错误文案自带「先 GET
   再建」契约。

### 白捡的两个发现

- **断连自动取消真机复现**：两次 poll 之间空窗超宽限期，SubscriberGuard 判断
  客户端断开 → 在飞的 spawn 轮被取消 → undo 自动擦掉 3 条半轮痕迹——M9 机制
  在 M17 现场顺手验了一遍。
- **文档/代码分歧（既有，非 M17 引入，已挂 ROADMAP §四）**：`existing`
  活会话带 `capabilities` 被**静默忽略**（200，代码自 062 起如此且注释写明
  理由——活会话换表撞红线 11），而 HOST-CAPABILITIES §三表格说「磁盘上有
  会话文件的一律拒绝」。对 `tools` 与 `prefix` 同样成立，待拍。
