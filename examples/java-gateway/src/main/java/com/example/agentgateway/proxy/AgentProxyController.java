package com.example.agentgateway.proxy;

import org.springframework.core.io.buffer.DataBuffer;
import org.springframework.http.server.reactive.ServerHttpRequest;
import org.springframework.http.server.reactive.ServerHttpResponse;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;
import org.springframework.web.reactive.function.BodyInserters;
import org.springframework.web.reactive.function.client.WebClient;
import org.springframework.web.server.ServerWebExchange;
import reactor.core.publisher.Mono;

import com.example.agentgateway.runtime.AgentServerProcess;

/**
 * `/agent/**` 除浏览器 SSE 之外的全部短请求转发：状态、输入、工具结果、
 * undo/redo 与显式 cancel。会话创建由 AgentSseController 按 chatid 幂等发起，
 * 不再由这条通配代理替浏览器生成随机 id。
 *
 * SSE 端点单独在 {@link AgentSseController}：Spring 按路径特异度派发，
 * 具体路径的 @GetMapping("/agent/sessions/{id}/events") 比这里的
 * `/agent/**` 通配优先命中，这个类不需要、也不应该手动排除 events 路径。
 */
@RestController
public class AgentProxyController {

    private final WebClient webClient;
    private final AgentServerProcess agentServer;

    public AgentProxyController(WebClient agentWebClient, AgentServerProcess agentServer) {
        this.webClient = agentWebClient;
        this.agentServer = agentServer;
    }

    @RequestMapping("/agent/**")
    public Mono<Void> proxy(ServerWebExchange exchange) {
        ServerHttpRequest request = exchange.getRequest();
        ServerHttpResponse response = exchange.getResponse();

        String upstreamPath = request.getPath().value().replaceFirst("^/agent", "");
        String rawQuery = request.getURI().getRawQuery();
        String uri = rawQuery == null ? upstreamPath : upstreamPath + "?" + rawQuery;

        return webClient.method(request.getMethod())
                .uri(agentServer.resolve(uri))
                // 全量转发，不逐个白名单复制（ARCHITECTURE.md「header 做全量
                // 转发」）。你的鉴权 filter 加在这条请求进网关之前：验完身份
                // 写进 X-Agent-User-Id，下面这一行原样把它带到 agent-server，
                // 网关本身不做任何校验（identity 只透传不验证，决策 11）。
                // traceparent（决策 11：只遵守 W3C，不集成 APM）同样靠这一行
                // 到达上游，不需要为它专门写代码。
                .headers(headers -> headers.addAll(HopByHopHeaders.strip(request.getHeaders())))
                // 用 DataBuffer 逐块转发请求体，不聚合成 String/byte[]——聚合
                // 式 body（比如 bodyToMono(String.class)）会等请求体完整收完
                // 才发往上游，这条路径未来可能扛较大的输入，提前用聚合会把
                // 流式优势在请求方向上先丢掉。
                .body(BodyInserters.fromDataBuffers(request.getBody()))
                .exchangeToMono(clientResponse -> {
                    response.setStatusCode(clientResponse.statusCode());
                    response.getHeaders().addAll(HopByHopHeaders.strip(clientResponse.headers().asHttpHeaders()));
                    // 响应体同样按 DataBuffer 逐块透传，理由同上。
                    return response.writeWith(clientResponse.bodyToFlux(DataBuffer.class));
                });
    }
}
