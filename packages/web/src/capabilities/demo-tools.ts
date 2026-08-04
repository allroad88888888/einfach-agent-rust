// 唯一职责：`web:demo/*` 这一组示例工具——**只有浏览器干得了的事**，声明与
// 实现放在一起（分开放两个文件才是真的会漂移：改了 schema 忘了改实现）。
//
// 挑这两个不是随便挑的：它们读的是**页面此刻的状态**，server 无论如何拿不到
// （没有第二条路能绕过前端），所以 068 真机验收「模型只能靠注入的工具完成
// 这个任务」时，模型不可能靠别的工具蒙混过去——这正是 068 §一要的那种任务。
//
// 两个都是 `"pure"`：只读 DOM/window，不写任何东西，重复调用结果一样，`/undo`
// 越过它们不需要问人（接缝 §五：宿主是企业自己的代码，它说 pure 就按 pure 办）。
//
// 执行时机不在本 issue（065 只管把声明发出去）——`pageTitle`/`viewport` 现在
// 没有调用方，**066 的派发**会按 `request.tool` 找到它们（经 `./index.ts` 的
// `findWebTool`）。返回值是 `string`，因为 `POST /tool_result` 的 body 是
// `{ content: string, is_error }`（066 范围条款 1）。
import type { CapabilityTool } from "@agent/protocol";

/** 无参工具的 schema——跟 Rust 侧无参工具同一个写法
 * （`agent-tools/src/command_discovery_specs.rs`）：`additionalProperties:
 * false` 让模型没有把参数扩出来的余地。 */
const NO_INPUT_SCHEMA = { type: "object", properties: {}, additionalProperties: false } as const;

export const PAGE_TITLE_TOOL = "web:demo/page-title";
export const VIEWPORT_TOOL = "web:demo/viewport";

export const demoTools: CapabilityTool[] = [
  {
    name: PAGE_TITLE_TOOL,
    description: "读取当前浏览器标签页的标题（document.title）。只有跑在这个页面里的前端拿得到，服务端没有任何办法得知。",
    schema: { ...NO_INPUT_SCHEMA },
    reversibility: "pure",
  },
  {
    name: VIEWPORT_TOOL,
    description: "读取当前浏览器视口的尺寸与像素比，返回 JSON：{width, height, dpr}。同样只有前端拿得到。",
    schema: { ...NO_INPUT_SCHEMA },
    reversibility: "pure",
  },
];

/** `web:demo/page-title` 的实现。空标题返回一句人话而不是空串——空的
 * `content` 到了模型那边跟「工具没返回东西」分不开，会让它以为调用失败。 */
export function pageTitle(): string {
  const title = document.title;
  return title === "" ? "(这个页面没有设置 <title>)" : title;
}

/** `web:demo/viewport` 的实现。 */
export function viewport(): string {
  return JSON.stringify({ width: window.innerWidth, height: window.innerHeight, dpr: window.devicePixelRatio });
}

/** 名字 → 实现。键必须跟上面 `demoTools` 里的 `name` 一一对应；`./index.ts`
 * 装配时会核对（对不上就在启动时炸，不留到模型真调用那一刻才发现）。 */
export const demoToolImpls: Record<string, () => string> = {
  [PAGE_TITLE_TOOL]: pageTitle,
  [VIEWPORT_TOOL]: viewport,
};
