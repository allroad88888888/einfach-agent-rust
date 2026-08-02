package com.example.agentgateway.config;

import org.springframework.boot.context.properties.ConfigurationProperties;

/**
 * 网关唯一的配置项：agent-server 的上游地址。
 *
 * application.yaml 里只有 agent.upstream 这一个 key，issue 037 明文要求
 * pom/配置都最小——不引入 profile、不引入配置中心 SDK。企业接自家配置
 * 中心时，唯一要动的地方就是这个值从哪来，网关代码本身不用改。
 */
@ConfigurationProperties(prefix = "agent")
public record GatewayProperties(String upstream) {

    private static final String DEFAULT_UPSTREAM = "http://127.0.0.1:4400";

    public GatewayProperties {
        if (upstream == null || upstream.isBlank()) {
            upstream = DEFAULT_UPSTREAM;
        }
    }
}
