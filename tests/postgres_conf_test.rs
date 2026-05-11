use pgforge::domain::platform::Platform;
use pgforge::domain::preset::Preset;
use pgforge::postgres::conf::generate_postgresql_conf;

#[test]
fn tiny_macos_conf_contains_full_fsync_writethrough() {
    let conf = generate_postgresql_conf(Preset::Tiny, Platform::MacOs);
    assert!(conf.contains("wal_sync_method = fsync_writethrough"),
            "expected fsync_writethrough on macOS, got:\n{conf}");
}

#[test]
fn linux_conf_uses_fdatasync() {
    let conf = generate_postgresql_conf(Preset::Tiny, Platform::Linux);
    assert!(conf.contains("wal_sync_method = fdatasync"));
}

#[test]
fn conf_always_contains_durability_settings() {
    for preset in [Preset::Tiny, Preset::Small, Preset::Medium, Preset::Large] {
        for plat in [Platform::MacOs, Platform::Linux] {
            let conf = generate_postgresql_conf(preset, plat);
            for must in [
                "fsync = on",
                "synchronous_commit = on",
                "full_page_writes = on",
                "wal_level = replica",
                "archive_mode = on",
                "archive_timeout = 60",
                "ssl = off",
                "password_encryption = scram-sha-256",
            ] {
                assert!(conf.contains(must), "preset={preset:?} plat={plat:?} missing {must:?}");
            }
        }
    }
}

#[test]
fn medium_conf_uses_medium_tuning() {
    let conf = generate_postgresql_conf(Preset::Medium, Platform::Linux);
    assert!(conf.contains("max_connections = 200"));
    assert!(conf.contains("shared_buffers = 1024MB"));
    assert!(conf.contains("effective_cache_size = 3072MB"));
    assert!(conf.contains("max_wal_size = 4096MB"));
}

#[test]
fn conf_uses_pgbackrest_archive_command() {
    let conf = generate_postgresql_conf(Preset::Tiny, Platform::Linux);
    assert!(conf.contains("archive_command = 'pgbackrest --stanza=main archive-push %p'"));
}
