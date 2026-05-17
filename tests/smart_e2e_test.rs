//! End-to-end SMART feature smoke test. Gated by `PGFORGE_E2E=1` because
//! it shells out to sudo, modifies /etc/sudoers.d, and writes systemd-user
//! units — not safe to auto-run in CI or local dev. Run manually on a
//! Linux box where you're OK setting up + tearing down the sudoers rule.
//!
//!     PGFORGE_E2E=1 cargo test --test smart_e2e_test -- --nocapture

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_check_uninstall_round_trip() {
    if std::env::var("PGFORGE_E2E").is_err() {
        eprintln!("skipping: PGFORGE_E2E not set");
        return;
    }
    // 1. install
    let health = pgforge::smart::install::install_all(
        pgforge::smart::install::InstallOpts { force: true }
    ).await.expect("install_all");
    assert!(!health.drives.is_empty(), "discovered at least one disk");

    // 2. check --write-cache (simulating timer)
    pgforge::commands::smart::run_check(true).await.expect("run_check");

    // 3. cache exists and is fresh
    let path = pgforge::smart::cache::default_cache_path();
    assert!(path.exists(), "cache file at {path:?}");
    let now = jiff::Timestamp::now();
    let h = pgforge::smart::cache::read_cache(
        &path, now, pgforge::smart::cache::STALE_AFTER_HOURS,
    );
    assert_ne!(h.unknown_reason, Some(pgforge::smart::types::SmartUnknownReason::NoCache));
    assert_ne!(h.unknown_reason, Some(pgforge::smart::types::SmartUnknownReason::Stale));

    // 4. uninstall
    pgforge::smart::install::uninstall_all().await.expect("uninstall_all");
    assert!(!path.exists(), "cache cleared by uninstall");
    assert!(!std::path::Path::new("/etc/sudoers.d/pgforge-smart").exists(),
            "sudoers fragment cleared by uninstall");
}
