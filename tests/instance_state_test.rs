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
        retain_days: 30,
                snapshot_hour: Some(3),
                last_snapshot_at: None,
                last_snapshot_attempt_at: None,
                full_backup_day: 0,
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
fn update_under_applies_and_persists_mutation() {
    let dir = TempDir::new().unwrap();
    let state_root = dir.path();
    fixture("billing").save_under(state_root).unwrap();
    let returned = InstanceState::update_under(state_root, "billing", |s| {
        s.instance.snapshot_hour = Some(7);
        Ok(())
    })
    .unwrap();
    assert_eq!(returned.instance.snapshot_hour, Some(7));
    let reloaded = InstanceState::load_under(state_root, "billing").unwrap();
    assert_eq!(reloaded.instance.snapshot_hour, Some(7));
}

#[test]
fn update_under_serializes_concurrent_mutations_without_lost_updates() {
    use std::thread;
    // Plain load->mutate->save races: two writers both read N, both write N+1,
    // one increment is lost. update_under re-loads inside the state-root lock,
    // so every increment lands.
    let dir = TempDir::new().unwrap();
    let state_root = dir.path().to_path_buf();
    let mut base = fixture("billing");
    base.instance.retain_days = 0;
    base.save_under(&state_root).unwrap();

    let mut handles = Vec::new();
    for _ in 0..8 {
        let root = state_root.clone();
        handles.push(thread::spawn(move || {
            InstanceState::update_under(&root, "billing", |s| {
                s.instance.retain_days += 1;
                Ok(())
            })
            .unwrap();
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    let final_state = InstanceState::load_under(&state_root, "billing").unwrap();
    assert_eq!(
        final_state.instance.retain_days, 8,
        "all 8 concurrent increments must survive — none clobbered"
    );
}

#[test]
fn backup_failing_false_when_never_ran() {
    let mut i = fixture("x").instance;
    i.last_snapshot_at = None;
    i.last_snapshot_attempt_at = None;
    assert!(!i.backup_failing());
}

#[test]
fn backup_failing_false_after_successful_snapshot() {
    // On success snapshot.rs writes both timestamps to the same value.
    let mut i = fixture("x").instance;
    i.last_snapshot_at = Some("2026-05-14T03:00:00Z".into());
    i.last_snapshot_attempt_at = Some("2026-05-14T03:00:00Z".into());
    assert!(!i.backup_failing());
}

#[test]
fn backup_failing_true_when_attempt_newer_than_last_success() {
    let mut i = fixture("x").instance;
    i.last_snapshot_at = Some("2026-05-13T03:00:00Z".into());
    i.last_snapshot_attempt_at = Some("2026-05-14T03:05:00Z".into());
    assert!(i.backup_failing(), "a failed attempt after the last good backup means backups are failing");
}

#[test]
fn backup_failing_true_when_attempted_but_never_succeeded() {
    let mut i = fixture("x").instance;
    i.last_snapshot_at = None;
    i.last_snapshot_attempt_at = Some("2026-05-14T03:05:00Z".into());
    assert!(i.backup_failing());
}

#[test]
fn backup_failing_false_when_backups_disabled() {
    let mut i = fixture("x").instance;
    i.backup_enabled = false;
    i.last_snapshot_at = None;
    i.last_snapshot_attempt_at = Some("2026-05-14T03:05:00Z".into());
    assert!(!i.backup_failing(), "a --no-backup instance is never 'failing'");
}

#[test]
fn save_under_locked_round_trips() {
    let dir = TempDir::new().unwrap();
    let state_root = dir.path();
    fixture("billing").save_under_locked(state_root).unwrap();
    let loaded = InstanceState::load_under(state_root, "billing").unwrap();
    assert_eq!(loaded.instance.name, "billing");
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
