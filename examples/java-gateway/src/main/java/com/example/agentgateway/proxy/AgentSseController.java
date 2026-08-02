package com.example.agentgateway.proxy;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;
import org.springframework.core.io.buffer.DataBuffer;
import org.springframework.http.HttpHeaders;
import org.springframework.http.MediaType;
import org.springframework.http.server.reactive.ServerHttpResponse;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PathVariable;
import org.springframework.web.bind.annotation.RequestHeader;
import org.springframework.web.bind.annotation.RestController;
import org.springframework.web.reactive.function.client.WebClient;
import org.springframework.web.server.ServerWebExchange;
import reactor.core.publisher.Flux;
import reactor.core.publisher.Mono;

/**
 * SSE 单列成一个控制器而不是并进 {@link AgentProxyController} 的通配转发，
 * 理由是 issue 037「SSE 透传三件事」：这条路径的正确性不能靠全量转发自动
 * 获得，需要三处显式动作（下面按「三件事·N」标号），少一个企业拷走就是
 * 坏的。三件事与 ARCHITECTURE.md「SSE 代理的四个坑」是两份相关但不同的
 * 清单（四个坑还包含「超时放开」，那一条在 WebClientConfig 里处理，不在
 * 这个类），下面的注释两份都会点名对应关系。
 */
@RestController
public class AgentSseController {

    private static final Logger log = LoggerFactory.getLogger(AgentSseController.class);

    private final WebClient webClient;

    public AgentSseController(WebClient agentWebClient) {
        this.webClient = agentWebClient;
    }

    @GetMapping("/agent/sessions/{id}/events")
    public Mono<Void> events(@PathVariable String id,
                              @RequestHeader(value = "Last-Event-ID", required = false) String lastEventId,
                              ServerWebExchange exchange) {
        ServerHttpResponse response = exchange.getResponse();

        Flux<DataBuffer> upstreamBody = webClient.get()
                .uri("/sessions/{id}/events", id)
                .accept(MediaType.TEXT_EVENT_STREAM)
                .headers(headers -> {
                    // 浏览器带来的其余头（identity/traceparent）原样透传，
                    // 理由同 AgentProxyController：全量转发，不专门写代码。
                    headers.addAll(HopByHopHeaders.strip(exchange.getRequest().getHeaders()));

                    // 三件事·2：Last-Event-ID 请求头透传（对应四个坑之外的
                    // 独立要求，031 那边靠它做补发）。重连补发靠 agent-server
                    // 读到这个头才知道该从哪一帧续发（031「Last-Event-ID
                    // 补发」）。EventSource 断线重连会自动带上它，这里如果
                    // 漏转，补发退化成 031 的「首连无 id」语义——从最旧可用
                    // 帧起补，而不是从断点续，不是这条链路想要的效果。
                    if (lastEventId != null) {
                        headers.set("Last-Event-ID", lastEventId);
                    }

                    // 四个坑 #2 · 不能压缩：对上游这条请求显式声明不接受
                    // 压缩编码。压缩会把事件攒到编码块边界才吐出来，SSE 从
                    // 「逐帧到达」退化成「攒一段才到」，等效于被缓冲——跟
                    // 三件事·1（不缓冲）是同一个症状的两个成因，这里堵的是
                    // 压缩这个成因，不缓冲那一半靠下面 DataBuffer 逐块转发。
                    headers.set(HttpHeaders.ACCEPT_ENCODING, "identity");
                })
                .exchangeToFlux(clientResponse -> {
                    response.setStatusCode(clientResponse.statusCode());
                    // 三件事·1 / 四个坑 #1：不缓冲。状态码和响应头原样透传，
                    // 包含 agent-server（031）已经发的两件套
                    // X-Accel-Buffering: no / Cache-Control: no-cache——
                    // 网关不需要重新设置，全量转发就带过去了。响应体本身
                    // 按 DataBuffer 逐块转发（不是 bodyToMono(String.class)
                    // 那种攒完整个流才发出的聚合读法），是「不缓冲」在
                    // WebFlux 里天然成立的原因：Flux 的每个元素到达就立刻
                    // 传给下一步，没有整体收完再转发这一环。四个坑 #3
                    // 「超时放开」不在这个方法里处理——WebClientConfig 里
                    // 那个 bean 刻意不设 responseTimeout，两处配合才完整。
                    response.getHeaders().addAll(HopByHopHeaders.strip(clientResponse.headers().asHttpHeaders()));
                    return clientResponse.bodyToFlux(DataBuffer.class);
                });

        // 三件事·3 / 四个坑 #4：断连传播。浏览器关掉 SSE 连接 → 底层
        // Reactor Netty 连接探测到对端关闭 → response.writeWith 订阅的
        // 响应体 Publisher 被取消 → 取消信号沿着这条 Flux 链路往上传到
        // exchangeToFlux 里对 agent-server 的这次 WebClient 调用，顺带
        // 取消对上游的订阅（agent-server 031 的宽限取消才会被触发，不
        // 白烧 token）。这个传播是 Reactor 的默认行为，不需要额外代码
        // 触发——但**别在这条链路上插 `.cache()` / `.share()`**，那两个
        // 操作符会把多个订阅者的取消信号合并、直到最后一个订阅者退出才
        // 真正取消上游，单订阅者场景下看不出问题，一旦有人为了「复用
        // 连接」顺手加一个，取消传播就悄悄失效了（ARCHITECTURE.md「SSE
        // 代理的四个坑」#4 原话）。doOnCancel 这里只打日志，不是取消
        // 发生的原因。
        return response.writeWith(
                upstreamBody.doOnCancel(() -> log.debug("SSE client disconnected, session={}", id)));
    }
}
