use pgforge::pgbackrest::conf::{S3Settings, generate_pgbackrest_conf};

fn s3_fixture() -> S3Settings {
    S3Settings {
        bucket: "pgforge-bk".into(),
        region: "eu-central-1".into(),
        endpoint: "s3.eu-central-1.amazonaws.com".into(),
        access_key: "AKIAFAKE".into(),
        secret_key: "secret".into(),
    }
}

#[test]
fn conf_includes_s3_repo_settings() {
    let conf = generate_pgbackrest_conf("billing", &s3_fixture());
    assert!(conf.contains("repo1-type=s3"));
    assert!(conf.contains("repo1-s3-bucket=pgforge-bk"));
    assert!(conf.contains("repo1-s3-region=eu-central-1"));
    assert!(conf.contains("repo1-s3-key=AKIAFAKE"));
    assert!(conf.contains("repo1-s3-key-secret=secret"));
}

#[test]
fn conf_namespaces_repo_path_per_instance() {
    let conf = generate_pgbackrest_conf("billing", &s3_fixture());
    assert!(conf.contains("repo1-path=/pgforge/billing"));

    let conf2 = generate_pgbackrest_conf("analytics", &s3_fixture());
    assert!(conf2.contains("repo1-path=/pgforge/analytics"));
}

#[test]
fn conf_has_main_stanza_with_local_socket() {
    let conf = generate_pgbackrest_conf("billing", &s3_fixture());
    assert!(conf.contains("[main]"));
    assert!(conf.contains("pg1-path=/var/lib/postgresql/data/pgdata"));
    assert!(conf.contains("pg1-user=pgbackrest"));
    assert!(conf.contains("pg1-socket-path=/var/run/postgresql"));
}

#[test]
fn conf_enables_async_archive_push_and_zstd() {
    let conf = generate_pgbackrest_conf("billing", &s3_fixture());
    assert!(conf.contains("archive-async=y"));
    assert!(conf.contains("compress-type=zst"));
    assert!(conf.contains("spool-path=/var/spool/pgbackrest"));
}
