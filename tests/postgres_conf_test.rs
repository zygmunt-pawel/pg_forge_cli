use pgforge::domain::preset::Preset;
use pgforge::postgres::conf::generate_postgresql_conf;

#[test]
fn conf_always_uses_fdatasync() {
    let conf = generate_postgresql_conf(Preset::Tiny);
    assert!(conf.contains("wal_sync_method = fdatasync"));
    assert!(!conf.contains("fsync_writethrough"));
}

#[test]
fn conf_always_contains_durability_settings() {
    for preset in [Preset::Tiny, Preset::Small, Preset::Medium, Preset::Large] {
        let conf = generate_postgresql_conf(preset);
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
            assert!(conf.contains(must), "preset={preset:?} missing {must:?}");
        }
    }
}

#[test]
fn medium_conf_uses_medium_tuning() {
    let conf = generate_postgresql_conf(Preset::Medium);
    assert!(conf.contains("max_connections = 200"));
    assert!(conf.contains("shared_buffers = 1024MB"));
    assert!(conf.contains("effective_cache_size = 3072MB"));
    assert!(conf.contains("max_wal_size = 4096MB"));
}

#[test]
fn conf_uses_pgbackrest_archive_command() {
    let conf = generate_postgresql_conf(Preset::Tiny);
    assert!(conf.contains("archive_command = 'pgbackrest --stanza=main archive-push %p'"));
}

#[test]
fn conf_with_archive_false_disables_archive_mode() {
    use pgforge::postgres::conf::generate_postgresql_conf_with_archive;
    let conf = generate_postgresql_conf_with_archive(Preset::Tiny, false);
    assert!(conf.contains("archive_mode = off"));
    assert!(!conf.contains("archive_mode = on"));
    assert!(!conf.contains("archive-push"));
}
