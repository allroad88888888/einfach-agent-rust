package com.example.agentgateway;

import com.example.agentgateway.config.GatewayProperties;
import org.springframework.boot.SpringApplication;
import org.springframework.boot.autoconfigure.SpringBootApplication;
import org.springframework.boot.context.properties.EnableConfigurationProperties;

/**
 * 入口。三步跑法见 README：mvn -q package -> 改 application.yaml 的
 * agent.upstream（默认已指向 127.0.0.1:4400，本地联调通常不用改）->
 * java -jar target/agent-gateway-0.0.0-reference.jar。
 *
 * 这个类刻意只做启动，不做别的事：鉴权 filter、日志采集、配置中心接入
 * 都是拷走这份代码之后企业自己加的东西，不在参考实现范围内——issue 037
 * 用户原话「丢掉鉴权丢掉日志，只实现主要功能」。
 */
@SpringBootApplication
@EnableConfigurationProperties(GatewayProperties.class)
public class AgentGatewayApplication {

    public static void main(String[] args) {
        SpringApplication.run(AgentGatewayApplication.class, args);
    }
}
