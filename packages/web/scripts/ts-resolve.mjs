// 唯一职责：让 `node` 能直接跑 `src/` 里那些**用无扩展名相对路径互相 import**
// 的 `.ts` 文件。
//
// 为什么需要它：Node 24 自带类型擦除（`.ts` 可以直接 `node xxx.ts`），但**不
// 改 ESM 解析规则**——`import "./client"` 在 Node 里就是找不到，必须写成
// `"./client.ts"`。而本仓 `packages/web/src/` 一律用无扩展名（`./api`、
// `./dom`、`./render/dispatch`），tsconfig 是 `moduleResolution: "Bundler"`。
// 两边只能靠一个解析钩子对上：**改源码去迁就测试是本末倒置**（那会让整个包
// 的 import 风格分裂，还要给 tsconfig 开 `allowImportingTsExtensions`）。
//
// 这样就不用为了跑几个断言往仓里装 vitest —— 见
// `docs/issues/067-frontend-mcp-client.md` 的实做记录。
import { registerHooks } from "node:module";

registerHooks({
  resolve(specifier, context, nextResolve) {
    try {
      return nextResolve(specifier, context);
    } catch (error) {
      // 只补相对路径：裸包名解析不出来是真的缺依赖，别掩盖。
      if (!specifier.startsWith(".")) throw error;
      // 两种写法都要补：`./client`（文件）和 `../capabilities`（目录，
      // bundler 解析成它的 index）。
      try {
        return nextResolve(`${specifier}.ts`, context);
      } catch {
        return nextResolve(`${specifier}/index.ts`, context);
      }
    }
  },
});
