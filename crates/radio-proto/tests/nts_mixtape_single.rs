mod common;

use common::nts_mixtape::{build_client, probe_single_url};

#[tokio::test]
async fn nts_mixtape_single_probe_reports_show_and_optional_url() {
    let url = std::env::var("NTS_MIXTAPE_URL")
        .unwrap_or_else(|_| "https://www.nts.live/infinite-mixtapes/slow-focus".to_string());

    let client = build_client().expect("client should initialize");
    let (alias, info, elapsed) = probe_single_url(&client, &url)
        .await
        .expect("single mixtape probe should succeed");

    println!("alias: {alias}");
    println!("show: {}", info.title.as_deref().unwrap_or("not announced"));
    println!("url: {}", info.url.as_deref().unwrap_or("(not announced)"));
    println!(
        "started_at: {}",
        info.started_at.as_deref().unwrap_or("(unknown)")
    );
    println!("elapsed_ms: {}", elapsed.as_millis());

    // Base validity: if URL exists, title should also exist.
    assert!(
        info.url.is_none() || info.title.is_some(),
        "episode URL exists but title is missing"
    );

    // Target check for current slow-focus validation run.
    if alias == "slow-focus" {
        let title_lower = info
            .title
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        assert!(
            title_lower.contains("longform editions"),
            "expected slow-focus title to include 'Longform Editions', got: {:?}",
            info.title
        );
    }
}
