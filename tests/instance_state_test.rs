use pgforge::domain::instance::Instance;
use pgforge::domain::preset::Preset;
use pgforge::state::instance::InstanceState;
use tempfile::TempDir;

fn fixture(name: &str) -> InstanceState {
    InstanceState {
        instance: Instance {
            name: name.into(),
            db_name: name.into(),
            app_user: "leads".into(),
            app_password: "pw".into(),
            pgbackrest_password: "rpw".into(),
            preset: Preset::Tiny,
            pg_version: 18,
            host_port: 5433,
            backup_enabled: true,
            volume_name_override: None,
        },
        created_at: "2026-05-11T08:00:00Z".into(),
    }
}

#[test]
fn instance_name_validation_rejects_uppercase() {
    let err = Instance::validate_name("Billing").unwrap_err();
    assert!(matches!(err, pgforge::error::PgForgeError::InvalidInstanceName(_)));
}

#[test]
fn instance_name_validation_accepts_alpha_numeric_underscore_dash() {
    Instance::validate_name("billing").unwrap();
    Instance::validate_name("billing-staging").unwrap();
    Instance::validate_name("billing_2").unwrap();
}

#[test]
fn instance_name_validation_rejects_starting_digit() {
    assert!(Instance::validate_name("2billing").is_err());
}

#[test]
fn save_then_load_round_trips() {
    let dir = TempDir::new().unwrap();
    let state_root = dir.path();
    let s = fixture("billing");
    s.save_under(state_root).unwrap();
    let loaded = InstanceState::load_under(state_root, "billing").unwrap();
    assert_eq!(s, loaded);
}

#[test]
fn list_returns_all_instances() {
    let dir = TempDir::new().unwrap();
    let state_root = dir.path();
    fixture("billing").save_under(state_root).unwrap();
    fixture("analytics").save_under(state_root).unwrap();
    let mut names = InstanceState::list_under(state_root).unwrap();
    names.sort();
    assert_eq!(names, vec!["analytics".to_string(), "billing".to_string()]);
}

#[test]
fn list_returns_empty_when_state_root_missing() {
    let dir = TempDir::new().unwrap();
    let state_root = dir.path().join("does-not-exist");
    let names = InstanceState::list_under(&state_root).unwrap();
    assert!(names.is_empty());
}

#[test]
fn load_legacy_state_without_backup_enabled_defaults_to_true() {
    // Instances created before P4A-1 don't have the backup_enabled field in
    // their state.toml. Loading them must default to true (pre-P4 instances
    // all had pgbackrest wired up, even if it was inert pre-bind-mount-fix).
    let dir = TempDir::new().unwrap();
    let state_root = dir.path();
    let instance_dir = state_root.join("instances").join("legacy");
    std::fs::create_dir_all(&instance_dir).unwrap();
    let legacy_toml = r#"created_at = "2026-05-11T08:00:00Z"

[instance]
name = "legacy"
db_name = "legacy"
app_user = "leads"
app_password = "pw"
pgbackrest_password = "rpw"
preset = "tiny"
pg_version = 18
host_port = 5499
"#;
    std::fs::write(instance_dir.join("state.toml"), legacy_toml).unwrap();
    let loaded = InstanceState::load_under(state_root, "legacy").unwrap();
    assert!(loaded.instance.backup_enabled, "missing field must default to true");
}
