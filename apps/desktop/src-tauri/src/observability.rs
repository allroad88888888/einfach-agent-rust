//! Desktop-host operational-log setup.
//!
//! This executable installs the process-global tracing subscriber before Tauri is built.
//! `RUST_LOG` controls filtering; setting `AGENT_LOG_FORMAT=json` selects JSON output,
//! while every other value keeps concise human-readable output.

use tracing_subscriber::{fmt, EnvFilter};

const DEFAULT_FILTER: &str = "info";

/// Installs the desktop process's only tracing subscriber.
///
/// Calling this twice is a host-programming error. Library crates only emit tracing events;
/// they never initialize a subscriber.
pub fn install() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));

    if json_requested() {
        fmt()
            .with_writer(std::io::stderr)
            .with_env_filter(filter)
            .json()
            .flatten_event(true)
            .with_current_span(true)
            .with_span_list(false)
            .with_ansi(false)
            .try_init()
            .expect("desktop tracing subscriber must be installed exactly once");
    } else {
        fmt()
            .with_writer(std::io::stderr)
            .with_env_filter(filter)
            .compact()
            .with_target(false)
            .try_init()
            .expect("desktop tracing subscriber must be installed exactly once");
    }
}

fn json_requested() -> bool {
    std::env::var("AGENT_LOG_FORMAT").is_ok_and(|format| format.eq_ignore_ascii_case("json"))
}
