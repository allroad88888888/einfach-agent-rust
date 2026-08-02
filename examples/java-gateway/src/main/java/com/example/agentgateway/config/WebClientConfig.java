package com.example.agentgateway.config;

import org.springframework.context.annotation.Bean;
import org.springframework.context.annotation.Configuration;
import org.springframework.web.reactive.function.client.WebClient;

/**
 * 指向 agent-server 的唯一 WebClient，两个控制器（AgentProxyController /
 * AgentSseController）共用同一个 bean。
 *
 * 刻意不调用 .responseTimeout(...)：Reactor Netty 的默认行为是这条连接不
 * 设响应超时。ARCHITECTURE.md「SSE 代理的四个坑」#3 点名的坑就是有人顺手
 * 加一句看起来无害的 .responseTimeout(Duration.ofSeconds(30))——SSE 是
 * 长连接，会在这个超时之后被硬切断，而这个调用一旦加上，报错现象是「聊
 * 到一半流断了」，很难联想到是这一行。拷走这份代码要给普通 REST 调用加
 * 超时，请另起一个 WebClient bean，不要改这一个。
 */
@Configuration
public class WebClientConfig {

    @Bean
    public WebClient agentWebClient(GatewayProperties properties) {
        return WebClient.builder()
                .baseUrl(properties.upstream())
                .build();
    }
}
