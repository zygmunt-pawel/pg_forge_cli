use pgforge::config::global::GlobalConfig;
use pgforge::pgbackrest::conf::S3Settings;
use tempfile::TempDir;

fn s3() -> S3Settings {
    S3Settings {
        bucket: "b".into(),
        region: "eu-central-1".into(),
        endpoint: "s3.eu-central-1.amazonaws.com".into(),
        access_key: "a".into(),
        secret_key: "s".into(),
    }
}

#[test]
fn load_returns_default_when_file_missing() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    let cfg = GlobalConfig::load_from(&path).unwrap();
    assert!(cfg.s3.is_none());
    assert_eq!(cfg.port_range_start, 5433);
    assert_eq!(cfg.port_range_end, 5500);
}

#[test]
fn save_then_load_round_trips() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("nested").join("config.toml");
    let cfg = GlobalConfig {
        s3: Some(s3()),
        port_range_start: 6000,
        port_range_end: 6100,
    };
    cfg.save_to(&path).unwrap();
    let loaded = GlobalConfig::load_from(&path).unwrap();
    assert_eq!(loaded, cfg);
}

#[test]
fn malformed_config_returns_typed_error() {
    use pgforge::error::PgForgeError;
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "this is not = valid toml [[[").unwrap();
    let err = GlobalConfig::load_from(&path).unwrap_err();
    assert!(matches!(err, PgForgeError::ConfigMalformed { .. }), "got {err:?}");
}
