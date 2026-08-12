# 158 真机收官 + 文档清账 ← M17 终点

**里程碑** M17 · **依赖** [156](156-server-prefix-declaration.md)（157 已后置，不阻塞本条） · **模型** 主会话前台 · **独测** 本条即验收 · **状态** 未开始

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
