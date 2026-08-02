# 037 Java WebFlux 参考网关

**里程碑** M4 · **依赖** 无 · **模型** sonnet · **独立测试 agent** 否 · **状态** 完成

## 目标

决策 13：参考实现，拷走改，不发 Maven 不跟版。企业拿它十分钟看懂「怎么把
agent-server 摆进自己的微服务体系」。用户原话的边界：**丢掉鉴权丢掉日志，
只实现主要功能**——他们自己加。

## 做什么

`examples/java-gateway/`（Maven 单模块，Spring Boot 3 WebFlux）：

- 反向代理 `/agent/**` → agent-server（地址配置化，默认 127.0.0.1:4400）
- **SSE 透传三件事**（README 里逐条讲why）：不缓冲（WebFlux Flux 流式转发天然
  逐块）、`Last-Event-ID` 请求头透传（重连补发靠它）、断连传播（下游断则断上游
  ——server 的宽限取消才能生效）
- identity 头透传示例（一行注释标明「你的鉴权在这里发生，然后把身份放进
  X-Agent-User 传下去」——不实现鉴权本身）
- `traceparent` 透传（决策 11：只遵守 W3C，不集成 APM）
- README：架构图（浏览器→网关→agent-server）、拷走改的三个常见动作
  （加鉴权过滤器、换配置中心、接自家日志）、与 K8s ClusterIP 部署的对应关系
  （ARCHITECTURE §部署形态）

## 验收（本机无 JDK——如实处理）

- 代码完整可读、pom 依赖最小、README 三步可跑（写清 JDK 17+ 前置）
- **本机构建验证缺席要在 README 与实做记录显著标注**：`mvn -q package` 未在
  开发机跑过，第一位企业用户就是它的 CI——参考实现的诚实边界
- 静态检查代替：SSE 转发的关键代码逐行注释、与 031 的六端点/头部两件套逐条对照

## 注意

不进任何 workspace（纯 examples 目录）；`.gitignore` 补 target/（Maven 的）。

## 实做记录（实现 agent，2026-08-02）

**本机没有 JDK，`mvn -q package` 全程没有跑过一次——这份记录里没有任何编译
或测试输出，因为它们不存在。** 静态自查（文件树逐一读过、逐文件行数核对、
与 031 路由表逐条对照、README 全文自查）代替构建验证，这是 issue 明文的
验收方式，不是省略号；下面每一节都是这次静态自查的结果。

### 落地

`examples/java-gateway/`（Maven 单模块，`spring-boot-starter-parent:3.3.4` +
`spring-boot-starter-webflux`），7 个 Java 源文件 + `pom.xml` +
`application.yaml` + `README.md`：

```
examples/java-gateway/
  pom.xml                                                  58 行，唯一 <dependency> 是 spring-boot-starter-webflux
  README.md                                               271 行
  src/main/resources/application.yaml                       4 行，唯一 key：agent.upstream
  src/main/java/com/example/agentgateway/
    AgentGatewayApplication.java                            24 行  入口
    config/GatewayProperties.java                           22 行  @ConfigurationProperties(prefix="agent")
    config/WebClientConfig.java                             27 行  唯一 WebClient bean + 超时坑注释
    proxy/HopByHopHeaders.java                              49 行  hop-by-hop 名单 + 过滤
    proxy/AgentProxyController.java                         61 行  031 路由表六条非-SSE 端点
    proxy/AgentSseController.java                          100 行  GET /sessions/:id/events，SSE 三件事单列
```

全部 ≤300 行（`one-file-one-thing` 精神：每个文件一句话说清是干嘛的），最长
的 `AgentSseController.java` 也只有 100 行，超出部分基本是解释「为什么」的
注释,不是逻辑本身的体积。

### 与 031 路由表逐条对照

| 031 端点 | 网关侧处理 | 备注 |
|---|---|---|
| `POST /sessions` | `AgentProxyController` 通配转发 | 剥 `/agent` 前缀 |
| `GET /sessions/:id` | 同上 | |
| `GET /sessions/:id/events` | **`AgentSseController` 单列** | SSE 三件事：不缓冲/`Last-Event-ID`/断连传播 |
| `POST /sessions/:id/input` | `AgentProxyController` | |
| `POST /sessions/:id/tool_result` | `AgentProxyController` | 501 原样透传，网关不拦截、不重新解释 |
| `POST /sessions/:id/undo` | `AgentProxyController` | |
| `POST /sessions/:id/redo` | `AgentProxyController` | |
| `POST /sessions/:id/cancel` | `AgentProxyController` | |

八条全覆盖（031「六端点」+ 会话创建/状态两条管理端点）。SSE 与非-SSE 分流靠
Spring 路径特异度自动派发（`@GetMapping` 具体路径优先于 `@RequestMapping`
通配），没有手写互斥逻辑。头部两件套（`X-Accel-Buffering: no` /
`Cache-Control: no-cache`）不由网关重新设置，靠 `AgentSseController` 的响应头
全量透传自然带过去。

### SSE 透传三件事的实现位置

- **不缓冲**：`AgentSseController` 用 `DataBuffer` 逐块转发（禁
  `bodyToMono(String.class)`），另加 `Accept-Encoding: identity` 堵压缩这个
  隐藏成因，`WebClientConfig` 里不设 `responseTimeout` 堵超时这个隐藏成因——
  三个成因两个文件配合才完整，README「SSE 透传三件事」一节把这层关系讲清楚。
- **`Last-Event-ID`**：显式读 `@RequestHeader` 再原样 `set` 到上游请求。
- **断连传播**：不需要额外代码——Reactor 的取消信号沿 `Flux` 链路自动往上
  传，`doOnCancel` 只用来打日志，注释里专门提醒不要插 `.cache()`/`.share()`
  挡住它。

### header 透传策略

identity 头（`X-Agent-User-Id` 等）与 `traceparent` 都没有专门代码：
`AgentProxyController`/`AgentSseController` 都是「全量转发，只挖
hop-by-hop」（`HopByHopHeaders` 类：`Connection`/`Keep-Alive`/
`Transfer-Encoding`/`Upgrade`/`TE`/`Trailer` + `Proxy-*` 前缀族），
identity/`traceparent` 不在名单里，自动带过去——对应 ARCHITECTURE.md
「专门写的代码是 0 行」原话，也对应决策 11。

### 依赖

`pom.xml` 唯一 `<dependency>` 是 `spring-boot-starter-webflux`（parent 用
`spring-boot-starter-parent:3.3.4` 管版本）。没加
`spring-boot-starter-test`/lombok/任何独立日志实现（Spring Boot 默认已带
Logback+SLF4J）。**没写单元测试**：没有 JDK 也没有 `mvn`，写了也没跑过，与其
留一份从没跑过的测试代码不如老实承认这条路当前走不通——跟诚实边界同一个
立场。

### `.gitignore`

`git check-ignore -v examples/java-gateway/target/classes/Foo.class`（临时
文件，验证后已清理）命中根 `.gitignore` 第 1 行的 `target/`——该规则无锚定，
在任意深度都生效，`examples/java-gateway/target/` 已经被覆盖。issue「注意」
一节要求的「`.gitignore` 补 target/」因此不需要新增规则，属于确认后说明，
不是需要编辑的缺口。

### 诚实边界（README 原文，供验收核对）

README「诚实边界：本机构建验证缺席」一节开头两句：

> 写这份代码的开发机上没有装 JDK。`mvn -q package` 没有在这个仓库里跑过一次
> ——一次都没有。代码是照 Spring WebFlux / Reactor 的公开 API 手写的，语法、
> 依赖坐标、方法签名靠人工核对，**没有编译器验证过、没有跑过一次请求**。

### 异议 / 未做的事

- **单元测试未写**：没有 JDK/`mvn` 可跑，写了也是从没跑过的代码，选择不写
  而不是假装做了半截（与 031 独测记录「如实记在这里而不是留一个不完整的
  实现」同一立场）。
- **未验证 Spring Boot 3.3.4 / `spring-boot-starter-webflux` 的具体 API
  签名在该版本上是否逐字符正确**（比如 `WebClient` 的
  `exchangeToFlux`/`exchangeToMono` 方法名、`ServerHttpRequest#getMethod()`
  返回类型是否为 `HttpMethod`）——这些是凭对 Spring WebFlux 的既有知识手写，
  真实的第一次验证是使用者的 `mvn -q package`。
- **未验证 SSE 断连传播在真实网络条件下的行为**（浏览器标签页关闭 / 网络
  中断 / 反向代理主动断开这几种断连方式，触发 Reactor 取消信号的路径是否
  完全一致）——参考实现的注释讲的是 Reactor 的既定契约（取消信号沿 `Flux`
  链路上传是文档化行为），不是这次跑出来的观测结果。

### 合并记录（主会话）

7 个 Java 源 + pom + README（271 行），与 031 路由表逐条对照齐。SSE 三件事
落点清楚（DataBuffer 流式 + Accept-Encoding: identity 防压缩缓冲、Last-Event-ID
透传、Reactor 取消传播 + 反 .cache() 警告注释）。诚实边界声明置顶采信——
「没有编译器验证过一次」写在 README 显著处，第一位企业用户就是它的 CI。
