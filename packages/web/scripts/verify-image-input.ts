// 唯一职责：086 的 HTTP 图片请求形状验收。对真的 `sendInput` 截获 fetch，断言
// 无图时请求体逐字节仍是旧的 `{\"text\":\"...\"}`，有图时才附带图片字节。
//
//   pnpm --filter web verify:images
//
// 浏览器入口、缩略图与 object URL 生命周期是 087 真浏览器 dogfood 的验收面；此
// 文件只钉住不依赖 DOM 的 API 契约，避免为这几个断言引入一套 DOM 测试框架。
import { sendInput } from "../src/api";

let passed = 0;
const failures: string[] = [];

function equal(label: string, actual: unknown, expected: unknown): void {
  if (JSON.stringify(actual) === JSON.stringify(expected)) {
    passed += 1;
    console.log(`  ✓ ${label}`);
    return;
  }
  failures.push(label);
  console.log(`  ✗ ${label} —— 实际 ${JSON.stringify(actual)}，期望 ${JSON.stringify(expected)}`);
}

async function main(): Promise<void> {
  const requests: Array<{ path: string; body: string | undefined }> = [];
  const realFetch = globalThis.fetch;
  globalThis.fetch = (async (input: string | URL | Request, init?: RequestInit) => {
    requests.push({ path: String(input), body: typeof init?.body === "string" ? init.body : undefined });
    return new Response(null, { status: 202 });
  }) as typeof fetch;

  try {
    await sendInput("plain", "北京天气");
    equal("无图 body 逐字节仍是旧形状", requests[0], {
      path: "/sessions/plain/input",
      body: '{"text":"北京天气"}',
    });

    const image = new File([new Uint8Array([137, 80, 78, 71])], "nonce.png", { type: "image/png" });
    await sendInput("image", "读数字", [image]);
    equal("有图才发送 name/mime/原始字节", requests[1], {
      path: "/sessions/image/input",
      body: '{"text":"读数字","images":[{"name":"nonce.png","mime":"image/png","bytes":[137,80,78,71]}]}',
    });
  } finally {
    globalThis.fetch = realFetch;
  }

  console.log(`\n=== ${passed} 条通过，${failures.length} 条失败 ===`);
  if (failures.length > 0) process.exitCode = 1;
}

void main();
