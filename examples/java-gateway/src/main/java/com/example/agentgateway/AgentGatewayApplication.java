package com.example.agentgateway;

import com.example.agentgateway.config.AgentRuntimeProperties;
import org.springframework.boot.SpringApplication;
import org.springframework.boot.autoconfigure.SpringBootApplication;
import org.springframework.boot.context.properties.EnableConfigurationProperties;

/**
 * 入口。三步跑法见 README：mvn -q package -> 设置 agent.runtime 与 provider
 * 环境变量 -> java -jar target/agent-gateway-0.0.0-reference.jar。Rust 运行时由
 * AgentServerProcess 起在 loopback 的随机端口，控制器不自己猜端口或管理进程。
 *
 * 这个类刻意只做启动，不做别的事：鉴权 filter、日志采集、配置中心接入
 * 都是拷走这份代码之后企业自己加的东西，不在参考实现范围内——issue 037
 * 用户原话「丢掉鉴权丢掉日志，只实现主要功能」。
 */
@SpringBootApplication
@EnableConfigurationProperties(AgentRuntimeProperties.class)
public class AgentGatewayApplication {

    public static void main(String[] args) {
        SpringApplication.run(AgentGatewayApplication.class, args);
    }
}
