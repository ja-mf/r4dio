mod common;

use common::nts_mixtape::{build_client, scan_all_aliases, DEFAULT_BOOTSTRAP_URL};
use std::time::Duration;

#[tokio::test]
async fn nts_mixtape_scan_all_reports_live_progress_and_summary() {
    let delay_secs = std::env::var("NTS_MIXTAPE_DELAY_SECONDS")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.40_f64);
    let max_aliases = std::env::var("NTS_MIXTAPE_MAX_ALIASES")
        .ok()
        .and_then(|s| s.parse::<usize>().ok());
    let bootstrap_url = std::env::var("NTS_MIXTAPE_BOOTSTRAP_URL")
        .unwrap_or_else(|_| DEFAULT_BOOTSTRAP_URL.to_string());

    let client = build_client().expect("client should initialize");
    let summary = scan_all_aliases(
        &client,
        Duration::from_secs_f64(delay_secs.max(0.0)),
        max_aliases,
        &bootstrap_url,
    )
    .await
    .expect("scan-all probe should succeed");

    println!("--- summary ---");
    println!("total: {}", summary.total);
    println!("with_url: {}", summary.with_url);
    println!("title_only: {}", summary.title_only);
    println!("not_announced: {}", summary.not_announced);
    println!("errors: {}", summary.errors);
    println!("elapsed_seconds: {:.2}", summary.elapsed.as_secs_f64());
    if summary.total > 0 {
        println!(
            "avg_seconds_per_alias: {:.2}",
            summary.elapsed.as_secs_f64() / summary.total as f64
        );
    }

    if let Some(slow_focus) = summary.rows.iter().find(|r| r.alias == "slow-focus") {
        println!(
            "slow-focus: show={} | url={} | started_at={} | elapsed_ms={}",
            slow_focus.data.title.as_deref().unwrap_or("not announced"),
            slow_focus.data.url.as_deref().unwrap_or("(not announced)"),
            slow_focus.data.started_at.as_deref().unwrap_or("(unknown)"),
            slow_focus.elapsed.as_millis()
        );
    }

    assert!(summary.total > 0, "expected at least one mixtape alias");
    assert_eq!(summary.errors, 0, "expected no per-alias errors");
}
