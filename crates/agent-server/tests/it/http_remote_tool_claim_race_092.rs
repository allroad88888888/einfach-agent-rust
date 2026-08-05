//! 092-D：两个真实 TCP/HTTP 客户端并发认领时，每轮只能有一个执行者获胜。

use std::sync::{Arc, Barrier};
use std::time::Duration;

use agent_core::Notice;
use agent_server::{Frame, SessionEvent, ToolTableSpec};
use serde_json::{Value, json};

use crate::support;
use crate::support::http_client;
use crate::support::server::{FakeServer, Script};

const ROUNDS: usize = 100;

fn browser_action_reply(round: usize) -> String {
    let call_id = format!("call_race_{round}");
    [
        format!(
            r#"data: {{"choices":[{{"index":0,"delta":{{"role":"assistant","content":null,"tool_calls":[{{"index":0,"id":"{call_id}","type":"function","function":{{"name":"browser_action","arguments":"{{\"action\":\"render_card\"}}"}}}}]}},"finish_reason":null}}]}}"#
        ),
        r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"tool_calls"}]}"#.to_owned(),
        "data: [DONE]".to_owned(),
        String::new(),
    ]
    .join("\n\n")
}

fn next_frame(sse: &mut http_client::SseReader) -> Frame {
    let raw = sse
        .next_event(Duration::from_secs(5))
        .expect("并发压测应在时限内收到事件");
    serde_json::from_str(&raw.data).unwrap_or_else(|error| panic!("{error}: {}", raw.data))
}

fn wait_for_remote_call(sse: &mut http_client::SseReader) -> (String, String) {
    loop {
        let frame = next_frame(sse);
        if let SessionEvent::ToolExecuting { call_id, request } = frame.event
            && &*request.tool == "browser_action"
        {
            return (frame.agent.0.to_string(), call_id.0.to_string());
        }
    }
}

fn wait_for_terminal(sse: &mut http_client::SseReader) {
    loop {
        if matches!(
            next_frame(sse).event,
            SessionEvent::Notice(Notice::TurnStatusChanged { status }) if status.is_terminal()
        ) {
            return;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_of_two_real_http_clients_wins_each_of_one_hundred_claim_races() {
    let scripts = (0..ROUNDS)
        .flat_map(|round| {
            [
                Script::Immediate(browser_action_reply(round)),
                Script::Immediate(support::wire::text_reply("已执行。")),
            ]
        })
        .collect();
    let upstream = FakeServer::start(scripts);
    let mut template = support::http_server::session_template(upstream.endpoint());
    template.tools = ToolTableSpec::Standard;
    let server = support::http_server::start_at_with_template(
        "127.0.0.1:0".parse().unwrap(),
        template,
        |config| config,
    )
    .await;

    for round in 0..ROUNDS {
        let created = http_client::request(server.addr, "POST", "/sessions", Some("{}"));
        assert_eq!(created.status, 201, "round {round}: {}", created.body);
        let id = support::extract_json_string_field(&created.body, "id");
        let (status, _, mut sse) =
            http_client::connect_sse(server.addr, &format!("/sessions/{id}/events"), None);
        assert_eq!(status, 200, "round {round}");
        let input = http_client::request(
            server.addr,
            "POST",
            &format!("/sessions/{id}/input"),
            Some(r#"{"text":"展示卡片"}"#),
        );
        assert_eq!(input.status, 202, "round {round}: {}", input.body);
        let (agent, call_id) = wait_for_remote_call(&mut sse);

        let barrier = Arc::new(Barrier::new(3));
        let contenders = ["browser-client", "java-client"].map(|claim_id| {
            let barrier = Arc::clone(&barrier);
            let path = format!("/sessions/{id}/tool_claim");
            let body = json!({
                "agent": agent,
                "tool_call_id": call_id,
                "claim_id": claim_id,
            })
            .to_string();
            let addr = server.addr;
            std::thread::spawn(move || {
                barrier.wait();
                (
                    claim_id,
                    http_client::request(addr, "POST", &path, Some(&body)),
                )
            })
        });
        barrier.wait();
        let outcomes = contenders.map(|thread| thread.join().expect("认领客户端不应 panic"));
        let statuses = outcomes.each_ref().map(|(_, response)| response.status);

        let winners: Vec<_> = outcomes
            .iter()
            .filter(|(_, response)| response.status == 200)
            .collect();
        let losers: Vec<_> = outcomes
            .iter()
            .filter(|(_, response)| response.status == 409)
            .collect();
        assert_eq!(winners.len(), 1, "round {round}: statuses={statuses:?}");
        assert_eq!(losers.len(), 1, "round {round}: statuses={statuses:?}");
        let loser: Value = serde_json::from_str(&losers[0].1.body).unwrap();
        assert_eq!(loser["error"]["code"], "tool_claimed_by_other");

        let winner_claim_id = winners[0].0;
        let result = json!({
            "agent": agent,
            "tool_call_id": call_id,
            "claim_id": winner_claim_id,
            "submission_id": format!("submission-{round}"),
            "outcome": { "status": "succeeded", "content": "done" },
        });
        let committed = http_client::request(
            server.addr,
            "POST",
            &format!("/sessions/{id}/tool_result"),
            Some(&result.to_string()),
        );
        assert_eq!(committed.status, 200, "round {round}: {}", committed.body);
        let committed: Value = serde_json::from_str(&committed.body).unwrap();
        assert_eq!(committed["disposition"], "committed");
        wait_for_terminal(&mut sse);
    }

    assert_eq!(upstream.request_count(), ROUNDS * 2);
}
