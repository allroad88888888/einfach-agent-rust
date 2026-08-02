# Java WebFlux 参考网关

> **拷走改，别当依赖。** 这不是一个要 `mvn install` 进你仓库依赖的库，是一份
> 「企业网关长什么样」的参考实现。不发 Maven 仓库、不承诺 Spring Boot 2/3
> 双兼容、不跟 `einfach-agent-rust` 的版本演进（[ROADMAP.md](../../docs/ROADMAP.md)
> 决策 13）。十分钟读完、拷进你自己的服务、按下面「拷走之后要做的三件事」改。

## 诚实边界：本机构建验证缺席

**写这份代码的开发机上没有装 JDK。`mvn -q package` 没有在这个仓库里跑过一次
——一次都没有。** 代码是照 Spring WebFlux / Reactor 的公开 API 手写的，语法、
依赖坐标、方法签名靠人工核对，**没有编译器验证过、没有跑过一次请求**。

这不是可以之后再补的免责声明，是这份参考实现的验收方式本身
（[issue 037](../../docs/issues/037-java-gateway.md)「验收」原文）：静态检查
代替构建，SSE 转发的关键代码逐行注释、与
[031 号 issue](../../docs/issues/031-http-sse.md) 的路由表逐条对照（见下方
「`/agent/**` 反向代理与 031 的路由表对照」），换掉本机构建验证缺席这件事。
**第一位真正跑
`mvn -q package` 的人，实质上就是这份代码的 CI**——遇到编译错误、依赖坐标
写错、API 用法过时，请按下面的路由表和注释自行核对/修正，这在拷走改的场景下
本来就是预期动作，而不是这份参考实现的意外失败。

## 架构图

```
┌──────────┐   HTTPS + SSE    ┌──────────────────┐   HTTP + SSE    ┌───────────────┐
│  浏览器   │ ───────────────▶ │ Java 网关 (本目录) │ ───────────────▶ │  agent-server  │
│ EventSource│ ◀─────────────── │ Spring WebFlux    │ ◀─────────────── │ (crates/agent- │
│  + fetch  │   SSE / JSON     │ WebClient 转发     │   SSE / JSON     │   server)      │
└──────────┘                  └──────────────────┘                  └───────────────┘
                                       │
                                       │ 企业在这里加：鉴权 filter、
                                       │ 配置中心接入、自家日志采集
                                       ▼
                              （这三件事分别见下方
                               「拷走之后要做的三件事」）
```

网关是**唯一**面向公网/企业内网暴露的一跳；`agent-server` 只监听
`127.0.0.1`（红线 8），没有鉴权、没有 TLS、没有集群感知——这些全部是网关（或
它前面的 Ingress/LB）的职责。详见下方「与 K8s 部署形态的对应关系」。

## 三步跑起来（前置：JDK 17+，本仓库未验证过这三步）

```bash
# 1. 打包（需要能访问 Maven Central；本机未跑过，坐标/依赖以 pom.xml 为准）
mvn -q package

# 2. 确认 agent-server 已经在跑（默认地址 127.0.0.1:4400，见 crates/agent-server）
#    不用改配置就是这个默认值；要换地址改 src/main/resources/application.yaml
#    的 agent.upstream，或用 --agent.upstream=http://... 覆盖

# 3. 起网关（默认监听 8080，Spring Boot 默认值，本项目未覆盖）
java -jar target/agent-gateway-0.0.0-reference.jar
```

之后浏览器打 `http://localhost:8080/agent/sessions/...`，网关剥掉 `/agent`
前缀转发到 `http://127.0.0.1:4400/sessions/...`。

## `/agent/**` 反向代理与 031 的路由表对照

[031 号 issue](../../docs/issues/031-http-sse.md) 落地的六个会话操作端点
+ 两个会话管理端点，全部靠前缀剥离原样转发。**没有一条端点被网关重新解释
语义**——网关不知道 `undo` 和 `redo` 的区别，它只知道「剥前缀、转发、把响应
原样吐回去」。

| 浏览器请求 | 网关转发到 agent-server | 处理位置 | 说明 |
|---|---|---|---|
| `POST /agent/sessions` | `POST /sessions` | `AgentProxyController` | 建会话，201 |
| `GET /agent/sessions/:id` | `GET /sessions/:id` | `AgentProxyController` | 状态查询 |
| `GET /agent/sessions/:id/events` | `GET /sessions/:id/events` | **`AgentSseController`（单列）** | SSE，见下节 |
| `POST /agent/sessions/:id/input` | `POST /sessions/:id/input` | `AgentProxyController` | 202 fire-and-forget |
| `POST /agent/sessions/:id/tool_result` | `POST /sessions/:id/tool_result` | `AgentProxyController` | 501（M3 未启用，网关原样透传这个 501） |
| `POST /agent/sessions/:id/undo` | `POST /sessions/:id/undo` | `AgentProxyController` | 202 |
| `POST /agent/sessions/:id/redo` | `POST /sessions/:id/redo` | `AgentProxyController` | 202 |
| `POST /agent/sessions/:id/cancel` | `POST /sessions/:id/cancel` | `AgentProxyController` | 202 |

`AgentSseController` 之所以单列成一个控制器而不是并进通配转发，是因为 SSE
这条路径的正确性不能靠「全量转发」自动获得——下一节展开。Spring 按路径
特异度派发请求：`@GetMapping("/agent/sessions/{id}/events")` 比
`@RequestMapping("/agent/**")` 更具体，自动优先命中，两个控制器之间不需要
（也没有）手写的互斥逻辑。

**头部两件套**（`X-Accel-Buffering: no` / `Cache-Control: no-cache`，031 的
`GET /events` 必发项）网关不重新设置——它们是 agent-server 发的响应头，
`AgentSseController` 做的是响应头全量透传（除 hop-by-hop），两件套跟着响应
一起原样过网关，到浏览器时还在。**这也是「透传不是一个功能，是不做过滤的
自然结果」在两件套上的具体体现**，跟 identity/traceparent 是同一个道理。

## SSE 透传三件事（issue 037 原话，逐条讲 why）

实现在 `src/main/java/com/example/agentgateway/proxy/AgentSseController.java`，
下面摘的是该文件里的注释，完整上下文以源码为准。

### 1. 不缓冲

WebFlux 的 `Flux<DataBuffer>` 流式转发天然逐块——**前提是没人把它聚合成
一次性的值**。`AgentSseController` 用 `clientResponse.bodyToFlux(DataBuffer.class)`
读上游响应体、`response.writeWith(...)` 写回浏览器，元素级透传：上游一个
`DataBuffer` 到达，立刻转发给浏览器，不等流结束、不等凑够一个缓冲区。

反例（**这份代码里没有**，写出来是让拷走的人知道要避免什么）：
`clientResponse.bodyToMono(String.class)` 会先把整个响应体读成一个 `String`
才返回——对 SSE 这种没有自然结束点的长连接，`bodyToMono` 根本等不到那个
「整个响应体」，实践中直接等价于永久挂起或者对客户端表现为「connect 了但一直
不吐数据」。

这一条还有第二个成因：压缩。gzip/deflate 会把输出攒到编码块边界才产出字节，
即使转发代码本身逐块处理，被压缩过的流在感知上也会退化成「攒一段才到」。
所以 `AgentSseController` 对上游请求显式发送 `Accept-Encoding: identity`，
堵住这第二个成因。

**超时是不缓冲的第三个隐藏成因，藏在 `WebClientConfig` 里而不是
`AgentSseController` 里**：`WebClient` 的 response timeout 一旦被设置
（哪怕看起来只是给「卡住的请求」兜底的合理数值），会在超时后硬切断连接——
对短请求这是正确行为，对 SSE 这种设计上就要开很久的长连接，会被误伤成
「用着用着流断了」。`WebClientConfig` 里这个 bean 刻意不调用
`.responseTimeout(...)`，注释里写明了原因和「要给别的调用加超时请另起一个
bean」的边界。

### 2. `Last-Event-ID` 请求头透传

浏览器 `EventSource` 断线重连时会自动带上 `Last-Event-ID: <上次收到的最后
一帧 id>`。这个头如果被网关吞掉，agent-server（031 落地的环形缓冲补发逻辑）
就只能按「首连无 id」处理——退化成从环形缓冲最旧可用帧开始补，而不是从断点
精确续发。`AgentSseController` 显式读这个请求头（Spring 的
`@RequestHeader(value = "Last-Event-ID", required = false)`）并原样设进
对上游的请求里。

### 3. 断连传播

浏览器关掉 SSE 连接 → 底层 Reactor Netty 连接探测到对端关闭 →
`response.writeWith(...)` 订阅的响应体 `Publisher` 被取消 → 取消信号沿着
`AgentSseController` 里那条 `Flux` 链路往上传，一路传到 `exchangeToFlux`
里对 agent-server 发起的这次 `WebClient` 调用，顺带取消掉对上游 SSE 的订阅。
`agent-server`（031 落地）看到订阅者清零，启动它自己的宽限期取消（默认 5s，
可配）——**这条链路走完，模型调用才会真正停止，不会因为浏览器关了标签页
就继续在后台烧 token**。

这个传播是 Reactor 的默认行为，本身不需要额外代码触发。真正需要写代码/写
注释提醒的是反过来的坑：**别在这条链路上插 `.cache()` / `.share()`**。这两
个操作符会把多个订阅者的取消信号合并、直到最后一个订阅者退出才真正取消
上游——单订阅者场景下测不出问题，一旦有人为了「给同一个 session 的多个标签
页复用一份上游连接」顺手加一个，取消传播就在这个人完全不知情的情况下失效
了。`AgentSseController` 里 `doOnCancel` 只用来打一行 debug 日志，**不是**
取消发生的原因，注释里专门强调了这一点以免被误读成「要写代码才能取消」。

## identity 头透传与 `traceparent`（决策 11）

**这份代码里找不到一行专门转发 `X-Agent-User-Id` 或 `traceparent` 的代码，
因为不需要写：`AgentProxyController` 和 `AgentSseController` 都是「全量转发
请求头/响应头，只挖掉 hop-by-hop 那几个」（`HopByHopHeaders` 类），identity
头和 `traceparent` 不在 hop-by-hop 名单里，自动跟着过去。**

- **identity**：你的鉴权 filter 加在请求进网关的路径上（Spring 的
  `WebFilter`，见下方「加鉴权过滤器」）。filter 验完身份之后把结果写进
  `X-Agent-User-Id` / `X-Agent-Tenant-Id` 这类头，`AgentProxyController` /
  `AgentSseController` 里全量转发的那一行会原样把它带到 agent-server——
  这份参考实现**不实现鉴权本身**，只保证鉴权 filter 产出的头不会在网关这
  一跳丢失。agent-server 侧的行为是「只读 identity header，不验证」
  （[ARCHITECTURE.md](../../docs/ARCHITECTURE.md) §边缘无关），读不到就落
  `anonymous`。
- **`traceparent`**：决策 11——server 只遵守 W3C `traceparent` 标准，不集成
  任何 APM SDK（SkyWalking/Sleuth/OTel 都能透传 `traceparent`，网关不需要
  认识它们）。同样靠全量转发自动过去，没有专门代码。

## 拷走之后要做的三件事

### 1. 加鉴权过滤器

在 `com.example.agentgateway` 下新增一个实现 `WebFilter` 的类，注册到过滤链
里、排在业务控制器之前，做完鉴权后把身份写进请求头（例如用
`ServerWebExchange.mutate().request(...)` 追加 `X-Agent-User-Id`），再放行。
`AgentProxyController` / `AgentSseController` 不用改——它们全量转发请求头，
新加的头自动跟着过去。参考 Spring Security 的 `SecurityWebFilterChain` 或手写
一个校验 JWT/Session 的 `WebFilter` 都可以，这份参考实现不预设你用哪种鉴权
体系。

### 2. 换配置中心

唯一的配置项是 `GatewayProperties`（`agent.upstream`），当前来自
`application.yaml`。接自家配置中心（Nacos/Apollo/K8s ConfigMap）通常只需要
让配置中心的 SDK 把这个 key 注入 Spring `Environment`（大部分配置中心客户端
都提供 `PropertySource` 适配），`GatewayProperties` 本身不用动一行。

### 3. 接自家日志

这份参考实现刻意不集成任何日志规范（决策 11：「server 不做鉴权/日志规范/
集群」，网关继承同样的立场）。`AgentSseController` 里唯一一行日志
（`doOnCancel` 里的 `log.debug`）用的是 SLF4J 门面（Spring Boot 默认绑定
Logback），接你们自己的日志采集通常是换 `logback-spring.xml` 里的
appender，代码不用动。

## 与 K8s ClusterIP 部署形态的对应关系

对应 [ARCHITECTURE.md](../../docs/ARCHITECTURE.md) §部署形态原文：

> server 独立跑，`replicas: 1` 起步。企业网关（他们自己的，或拷走
> `examples/java-gateway/`）挡在前面，server 只有 ClusterIP、不开 Ingress。

翻译成这份参考实现的部署形态：

```
Ingress / 企业内网 LB
      │
      ▼
┌─────────────────┐   Service (ClusterIP，不开 Ingress)
│ 本目录打的镜像    │──────────────────────────────┐
│ Deployment       │                              ▼
│ replicas: N       │                    ┌───────────────────┐
│（网关是无状态的，   │                    │ agent-server        │
│ 可以水平扩）        │                    │ Deployment           │
└─────────────────┘                    │ replicas: 1 起步      │
                                        │ Service: ClusterIP   │
                                        │ 不开 Ingress          │
                                        └───────────────────┘
```

- **`agent-server` 只有 `ClusterIP`，不开 `Ingress`**——它默认绑
  `127.0.0.1`（红线 8，`AGENT_BIND` 显式才准 `0.0.0.0`），集群内 Pod 间通信
  走 Service DNS 才需要监听 `0.0.0.0`，这个开关留给部署清单去设，不是网关
  的职责。
- **网关是唯一开 `Ingress`（或对企业内网可达）的一跳**，`agent.upstream`
  配置项指向 `agent-server` 的 Service DNS 名（比如
  `http://agent-server.<namespace>.svc.cluster.local:4400`），不是
  `127.0.0.1`——本地跑的默认值只在网关和 agent-server 同机联调时有意义。
- **网关本身是无状态的**（不持有任何 session 状态，纯转发），`replicas`
  可以按流量水平扩，不像 `agent-server` 当前单副本起步——`agent-server`
  的多副本自路由（[ARCHITECTURE.md](../../docs/ARCHITECTURE.md)
  §多副本时的粘性路由，`RedisRegistry`）落地之前，`agent-server` 保持
  `replicas: 1`，网关这一层的扩容跟它无关。
- **网关不做会话粘性路由**：`GET /agent/sessions/:id/events` 落到哪个网关
  副本都一样，因为网关本身不记录任何东西，每个请求独立转发到
  `agent.upstream` 指向的同一个 Service。粘性问题（这个 session 的 actor
  线程在哪个 `agent-server` Pod 上）是 `agent-server` 自己未来要解的，不是
  网关的职责——这也是为什么网关代码里完全没有出现 session 亲和性相关的
  逻辑。

## pom 依赖

只有一个 `<dependency>`：`spring-boot-starter-webflux`。**必须用 WebFlux**：
Spring MVC 的 `SseEmitter` 一个连接占一个 Tomcat 线程，默认
`max-threads: 200` 意味着两百个并发会话就把整个应用的线程池吃光，连普通
接口都开始排队（[ARCHITECTURE.md](../../docs/ARCHITECTURE.md)「必须用
WebFlux」原话）。企业存量大多是 Spring MVC，接这份参考实现时如果整个应用
不方便迁 WebFlux，出路是给 `spring.mvc.async` 配独立线程池，或者把这条链路
单独拆一个 WebFlux 服务——不要在 MVC 应用里直接用 `SseEmitter` 顶这个流量。

## 目录结构

```
examples/java-gateway/
  pom.xml
  README.md                          本文件
  src/main/resources/
    application.yaml                 唯一配置项：agent.upstream
  src/main/java/com/example/agentgateway/
    AgentGatewayApplication.java     入口，20 行量级
    config/
      GatewayProperties.java         @ConfigurationProperties(prefix = "agent")
      WebClientConfig.java           唯一的 WebClient bean，超时坑的注释在这
    proxy/
      HopByHopHeaders.java           hop-by-hop 头部名单 + 过滤，两个 controller 共用
      AgentProxyController.java      031 路由表里除 events 之外的六条
      AgentSseController.java        GET /sessions/:id/events，SSE 三件事单列在这
```

七个源文件，每个文件一个职责（`one-file-one-thing`：能一句话说清是干嘛的），
全部 ≤300 行——最长的 `AgentSseController.java` 也只有 100 行左右，超出部分
基本都是解释「为什么」的注释，不是逻辑本身的体积。
