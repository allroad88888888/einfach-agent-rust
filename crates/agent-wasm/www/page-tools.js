// page-tools.js —— 页面这一侧的工具表：**声明什么，以及怎么执行**。
//
// issue 122 定的入口（`new AgentHost(configJson, toolDeclarationJson)`）+ issue 121
// 定的回调（`(name, inputJson) => Promise<string>`）。两者放同一个文件是 122 实做
// 记录第 4 条那条判断的直接后果：**谁实现谁声明**——把一条工具的描述和它的实现
// 拆到两处住，描述写歪没有任何人报错，模型照着错描述用工具。
//
// 从 index.html 摘出来是 issue 130 的一部分（红线 9：那个文件加东西之前 339 行，
// 已经超了）。摘的这一刀落在「工具」这个职责上，不是按行数切的。
//
// ⚠️ 红线 11 的责任在这里，不在 Rust：工具表进 prompt **最前面**，刷新前后必须
// 逐字节一样，否则前缀缓存全断（DeepSeek 上 120 倍差价，而且功能完全正常、一声
// 不吭）。运行时只帮两件事——数组按名字排序、schema 的键序归一；帮不了字段顺序
// 和描述文案。所以下面是**模块级常量字符串**，不是每次现拼的字面量：常量的字节
// 由源文件定死，不可能因为哪次刷新的分支不同而漂。完整规则见 AgentHost 构造函数
// 的文档注释（生成的 pkg/agent_wasm.d.ts 里能直接读到）。
//
// **本条又把工具表的字节改了一次**（多一条 `web:source/vision`），旧会话第一轮的
// 前缀缓存会再断一次——121 实做记录第 4 条、122 实做记录第 7 条各预告过一次，
// 这是第三次，也是 M14 计划内的最后一次。

import { runVisionTool } from "./vision-tool.js";

/** 三条 Rust 内建（`crate::tools`）。页面**连声明都不许**（撞名在 `tools::declare`
 * 就被拒），这里只用来做 121 那条「内建优先」反向锁的哨兵判断。 */
const BUILTIN_NAMES = ["web:page/title", "web:page/url", "web:source/echo"];

/** 130 的正身。`web:source/` 前缀是**故意的**：它自动激活 transient-source
 * 那一整套（119 §三——入参在历史里换成占位符、结果被 SAFE_RESULT 遮掉、真正的
 * 正文只 one-shot 覆盖进 prompt 一次）。 */
const VISION_TOOL = "web:source/vision";

// 这个页面自己实现、交给模型用的那一段工具表。三条：
//
//   · web:host/callback-probe —— 121 的验收脚手架。它**以前硬编码在 Rust 的
//     tools.rs 里**，122 之后改由这份声明提供，描述与 schema 逐字未改；121 的四条
//     真机验收因此原样仍然成立。这是「工具表真的从页面来了」最直接的自证。
//   · web:page/viewport —— 一条真·页面能力：视口尺寸，服务端算不出来。
//   · web:source/vision —— 130 的正身，浏览器识图。
//
// ⚠️ vision 那条的 description 与 schema **照抄 `agent-tools/src/vision_inspect.rs`
// 的 `vision_inspect_spec()`**，只改两处，别重写：
//   ① 名字（`srv:vision/inspect` → `web:source/vision`）；
//   ② 「或本机相对路径」去掉——浏览器里没有本机文件系统这回事。这句话在原文里
//      出现两次（description 里的「或本机相对路径」、schema 里 image 那条的
//      「或相对路径」），**两处一起去掉**：只去一处等于仍然告诉模型可以传相对
//      路径，它照做就是一次必然失败的调用。
// 剩下每一个字都是原样。它是进 prompt 的字节，而且现有那份是真机验过模型能照着
// 用的——重写一遍等于把一份验过的提示词换成一份没验过的。
//
// reversibility 落 irreversible，跟 native 那条一致（`vision_inspect.rs:66-68`：
// 调第三方 API 计费，undo 不该重放）。
export const PAGE_TOOL_DECLARATION = `{"tools":[
  {"name":"web:host/callback-probe",
   "description":"验收脚手架：由页面自己的工具回调执行，返回一句页面生成的字符串（含它实际花掉的毫秒数）。不是给模型日常使用的能力。无参数。",
   "schema":{"type":"object","properties":{},"additionalProperties":false},
   "reversibility":"pure"},
  {"name":"web:page/viewport",
   "description":"读取当前浏览器窗口的视口尺寸（宽×高，CSS 像素）与设备像素比。这个信息只有跑在页面里的宿主拿得到。无参数。",
   "schema":{"type":"object","properties":{},"additionalProperties":false},
   "reversibility":"pure"},
  {"name":"web:source/vision",
   "description":"识别一张本地图片的内容，返回文字描述。image：本地图片链接地址，必填——只接受本地上传返回的链接（形如 /uploads/<id>），不接受公网 URL。question：想问这张图的问题，可选，缺省为“这张图片里有什么？”。图片字节只发给识图服务（Kimi 3），不会进入对话历史或模型上下文，你只会拿到识别结果的文本。",
   "schema":{"type":"object","properties":{"image":{"type":"string","description":"本地图片链接（/uploads/<id>），必填。"},"question":{"type":"string","description":"识别问题，可选，缺省为“这张图片里有什么？”。"}},"required":["image"],"additionalProperties":false},
   "reversibility":"irreversible"}
]}`;

// 122 的反向锁：srv: 是「服务端进程内执行」的前缀，页面声明它等于给自己开一件
// 本进程根本没有实现的工具。必须被**当场拒**（建宿主就失败并点名），不是默默
// 接受，也不是默默把这一条丢掉、剩下的照用。
export const REJECTED_TOOL_DECLARATION = `{"tools":[
  {"name":"srv:shell/exec","description":"页面不该能声明这个。","reversibility":"pure"}
]}`;

/**
 * 建一条工具执行回调（121 的 `onToolCall(handler)` 收的就是它）。
 *
 * 一个回调里同时是好几条验收的证据面：
 *   ① 可等待——probe 那支真的 await 500ms，把实测毫秒数写进返回值和事件流，
 *      「500ms 真的过去了」因此是可观测的（同步执行不可能报出 ≥500）；
 *   ② 反向锁·reject——勾上「让它 throw」，这一轮不崩页面，模型收到 is_error 自纠；
 *   ③ 反向锁·没装回调——建宿主时不勾「装上」，模型调 probe 拿到 Failure，不挂死；
 *   ④ 内建优先——对三条内建名字返回哨兵。哨兵**永远不该**出现在模型的答案里，
 *      出现了就说明回调劫持了内建；
 *   ⑤ 130——`web:source/vision` 落到 runVisionTool。
 *
 * ⚠️ 回调里只能干「不经过这个 AgentHost 的活」：await、fetch、IndexedDB、DOM、
 * 以及 `inspectImage()`（**它不碰 `live`，识图这条路正是这么走的**）。回头调
 * `host.send()` / `openSession()` / `sessionId()` / `historyJson()` 是
 * already borrowed panic，理由见 onToolCall 的文档注释。
 *
 * @param {object} deps
 * @param {(line: string) => void} deps.log 事件流日志
 * @param {() => boolean} deps.shouldThrow 「让它 throw」勾选框的当前状态
 * @param {(link: string) => Promise<{bytes: Uint8Array, mime: string}>} deps.resolveImage 129
 * @param {(bytes: Uint8Array, mime: string, question: string) => Promise<string>} deps.inspectImage 127
 * @returns {(name: string, inputJson: string) => Promise<string>}
 */
export function createToolCallback({ log, shouldThrow, resolveImage, inspectImage }) {
  return async function onToolCall(name, inputJson) {
    const started = performance.now();
    log(`[tool-callback] ← ${name} input=${inputJson}`);
    if (BUILTIN_NAMES.includes(name)) {
      return "SENTINEL-页面回调劫持了内建工具（不该出现在模型答案里）";
    }
    // 130：入参是认领之后的真值（transient-source 只遮历史/prompt，不遮执行）。
    // 失败一律以 VisionToolError throw 出去 → 121 的 reject 路径 → 模型收到一条
    // `[code] …` 的 is_error 结果，形状跟 native 的 srv:vision/inspect 一样。
    if (name === VISION_TOOL) {
      const text = await runVisionTool({ inputJson, resolveImage, inspectImage });
      log(`[tool-callback] → 识图返回 ${text.length} 字符`);
      return text;
    }
    // 122 的自定义工具：一条真·只有页面算得出来的能力，服务端形态的 agent 无论
    // 如何都拿不到。声明在 PAGE_TOOL_DECLARATION，执行在这里——「页面声明 + 页面
    // 执行」这条路的最小完整样本。
    if (name === "web:page/viewport") {
      return `视口 ${window.innerWidth}×${window.innerHeight} CSS 像素，devicePixelRatio=${window.devicePixelRatio}。`;
    }
    if (shouldThrow()) {
      log('[tool-callback] → throw new Error("boom")');
      throw new Error("boom");
    }
    await new Promise((r) => setTimeout(r, 500));
    const elapsed = Math.round(performance.now() - started);
    log(`[tool-callback] → 真的等了 ${elapsed} 毫秒`);
    return `页面回调执行完毕，口令 PAGE-CALLBACK-OK，实测等待 ${elapsed} 毫秒。`;
  };
}
