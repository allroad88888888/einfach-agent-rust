package com.example.agentgateway.config;

import org.springframework.context.annotation.Bean;
import org.springframework.context.annotation.Configuration;
import org.springframework.web.reactive.function.client.WebClient;

/**
 * 指向 agent-server 的唯一 WebClient。实际 loopback 地址在 agent-server 写出
 * ready file 之后才知道，所以不能在这里提前设 baseUrl；调用方从
 * AgentServerProcess 取已握手的绝对 URI。
 *
 * 刻意不调用 .responseTimeout(...)：Rust poll 可以按 poll-wait-ms 挂起约
 * 25 秒，网关随后立即发下一次。不要给这个 client 加一个比长轮询更短的响应
 * 超时，否则会把正常空等误判为失败；普通 REST 调用需要不同超时时另起 bean。
 */
@Configuration
public class WebClientConfig {

    @Bean
    public WebClient agentWebClient() {
        return WebClient.builder().build();
    }
}
