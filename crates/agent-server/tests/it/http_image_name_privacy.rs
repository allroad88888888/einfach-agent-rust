//! 093：图片名只允许安全 basename，路径信息不得越过 HTTP 边界。

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use agent_providers::kimi::Kimi;
use agent_server::{AgentServer, ServerConfig, SessionTemplate, SessionsHandle};
use serde_json::json;

use crate::image_upload_upstream::{ImageUploadUpstream, UploadReply};
use crate::support;
use crate::support::http_client;

const PNG_HEADER: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

#[tokio::test(flavor = "multi_thread")]
async fn path_shaped_image_names_are_rejected_without_echo_or_provider_io() {
    let upstream = ImageUploadUpstream::start(UploadReply::Ok);
    let sessions_dir = support::temp_dir("image-name-privacy");
    let (addr, sessions) = start(template(&upstream, Some(sessions_dir.clone()))).await;
    create(addr, "invalid-image-names");

    for (name, canary) in [
        ("/private/POSIX_NAME_CANARY.png", "POSIX_NAME_CANARY"),
        ("../TRAVERSAL_NAME_CANARY.png", "TRAVERSAL_NAME_CANARY"),
        (
            r"C:\private\WINDOWS_DRIVE_NAME_CANARY.png",
            "WINDOWS_DRIVE_NAME_CANARY",
        ),
        (r"\\server\share\UNC_NAME_CANARY.png", "UNC_NAME_CANARY"),
    ] {
        let response = post_image(addr, "invalid-image-names", name);
        assert_eq!(response.status, 400, "accepted path-shaped name {name:?}");
        assert!(
            !response.body.contains(name) && !response.body.contains(canary),
            "rejection echoed private image name {name:?}: {}",
            response.body
        );
    }

    assert_eq!(upstream.upload_count(), 0, "rejected names reached upload");
    assert_eq!(upstream.chat_count(), 0, "rejected names reached chat");
    assert!(
        sessions
            .close_all()
            .iter()
            .all(|(_, result)| result.is_ok())
    );

    let journal =
        std::fs::read_to_string(sessions_dir.join("invalid-image-names.jsonl")).unwrap_or_default();
    for canary in [
        "POSIX_NAME_CANARY",
        "TRAVERSAL_NAME_CANARY",
        "WINDOWS_DRIVE_NAME_CANARY",
        "UNC_NAME_CANARY",
    ] {
        assert!(
            !journal.contains(canary),
            "journal leaked {canary}: {journal}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_unicode_basename_is_accepted_and_materialized() {
    let upstream = ImageUploadUpstream::start(UploadReply::Ok);
    let (addr, sessions) = start(template(&upstream, None)).await;
    create(addr, "unicode-image-name");

    let response = post_image(addr, "unicode-image-name", "截图 2026-08-06.png");
    assert_eq!(
        response.status, 202,
        "Unicode basename rejected: {}",
        response.body
    );
    wait_for_provider_calls(&upstream);
    assert_eq!(upstream.upload_count(), 1);
    assert_eq!(upstream.chat_count(), 1);
    assert!(
        sessions
            .close_all()
            .iter()
            .all(|(_, result)| result.is_ok())
    );
}

async fn start(template: SessionTemplate) -> (SocketAddr, SessionsHandle) {
    let server = AgentServer::new(
        ServerConfig::new(template)
            .with_private_capability(support::http_server::PRIVATE_CAPABILITY),
    );
    let sessions = server.sessions();
    let bound = server
        .bind("127.0.0.1:0".parse().expect("loopback address"))
        .await
        .expect("bind image-name test server");
    let addr = bound.local_addr();
    tokio::spawn(bound.serve());
    tokio::time::sleep(Duration::from_millis(20)).await;
    (addr, sessions)
}

fn template(
    upstream: &ImageUploadUpstream,
    sessions_dir: Option<std::path::PathBuf>,
) -> SessionTemplate {
    let mut template = support::http_server::session_template(upstream.chat_endpoint());
    template.upload_base_url = upstream.upload_base_url();
    template.provider = std::sync::Arc::new(Kimi);
    template.model = std::sync::Arc::from("kimi-for-image-name-test");
    template.api_key = "test-api-key".to_string();
    template.default_sessions_dir = sessions_dir;
    template
}

fn create(addr: SocketAddr, id: &str) {
    let response = http_client::request(
        addr,
        "POST",
        "/sessions",
        Some(&json!({ "id": id }).to_string()),
    );
    assert_eq!(response.status, 201, "create {id}: {}", response.body);
}

fn post_image(addr: SocketAddr, id: &str, name: &str) -> http_client::HttpResponse {
    let body = json!({
        "text": "inspect",
        "images": [{ "name": name, "mime": "image/png", "bytes": PNG_HEADER }]
    });
    http_client::request(
        addr,
        "POST",
        &format!("/sessions/{id}/input"),
        Some(&body.to_string()),
    )
}

fn wait_for_provider_calls(upstream: &ImageUploadUpstream) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if upstream.upload_count() == 1 && upstream.chat_count() == 1 {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!(
        "provider calls did not finish: {} upload, {} chat",
        upstream.upload_count(),
        upstream.chat_count()
    );
}
