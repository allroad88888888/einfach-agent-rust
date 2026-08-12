# 148 扩展包接缝：ExtensionPack 定型

**里程碑** M16 · **依赖** [146](146-intercept-registry.md) · **模型** **opus** · **独测** ✅ · **状态** 待做

## 目标

给「一个 Rust 扩展」定一个**交付物形状**：一个结构体打包它要带进会话的
全部东西，宿主装配一次吃一包。接缝错了不会红——只会在第三方接入的第一天
以「什么都得改内核」的形式浮出来，所以是 opus 级（025 接缝定型同判据）。

## 拍板过的边界（决策 29，不重议）

- 扩展 = Rust；内核零脚本运行时；TS 生态在宿主层（web-agent）。
- 扩展工具访问状态只有截获一条正门（Session 手套）；纯 IO 工具走 executor
  形状但**统一经截获注册表接入**（不碰 Session 就别碰，签名一致省一套机制）。

## 做什么

1. 定形状（名字可议，字段面如下）：
   ```rust
   pub struct ExtensionPack {
       pub name: Arc<str>,                              // 授权/日志用
       pub tools: Vec<(ToolSpec, InterceptFn)>,         // 声明 + 执行体成对
       pub timed: Vec<(ToolSpec, CallTiming, TimedRun)>,// 开局/收尾
   }
   ```
   成对交付是要点：**spec 与执行路径同进同出**（147 的半开教训直接写进
   类型）。
2. 装配：`ToolTable`/`RunnerCtx` 的 builder 加「吃一包」的入口——specs 进表
   （走 push_spec 判重）、截获进注册表、timed 进 timed 区。宿主
   （cli/server）以包为粒度授权：装不装某个包 = 一行。
3. 命名空间：扩展工具名强制 `ext:<pack>/<tool>` 前缀（照 M10 强制 `web:`/
   `desk:` 的同一条理由——结构上不与内置撞名，069「能靠命名让撞名不可能，
   就不写策略」）。`location_of` 对 `ext:` 落 `Server`。
4. 可逆性：包内工具的 `Reversibility` 随 pack 显式声明（照 `with_mcp` 的
   `(spec, reversibility)` 模式），缺省 `Irreversible`——不猜。
5. 文档：`docs/EXTENSIONS.md` 新接缝文档（M16 的接缝档案）：形状、正门
   （Session 手套的能与不能）、纪律（后代收窄、command 写、逐字节）、
   与 MCP / 宿主 capabilities 的分工表。

## 验收

- 一个测试用 pack（两个工具：一纯读截获 + 一 TurnEnd hook）经装配入口
  进来，模型脚本化调用全通；不装这个包的会话逐字节零变化。
- `ext:` 前缀强制：裸名/冒用 `srv:` 的 pack 装配期被拒（debug_assert +
  看门狗测试）。
- 可逆性缺省 Irreversible：pack 不声明 → undo 撞上会停（既有屏障机制）。

## 注意

- 红线 11：pack 的 specs 进表即进 prompt，装配顺序 = 宿主给包的顺序，
  逐字节确定。
- 别做动态加载（dylib/so）——扩展是编译期依赖，谁要动态化谁将来单开
  issue 论证（信任模型完全不同）。
