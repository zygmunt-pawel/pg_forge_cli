use pgforge::commands::restore::restored_instance_state;
use pgforge::domain::instance::Instance;
use pgforge::domain::preset::Preset;
use pgforge::state::instance::InstanceState;

fn source_state() -> InstanceState {
    InstanceState {
        instance: Instance {
            name: "src".into(),
            db_name: "leads".into(),
            app_user: "app".into(),
            app_password: "pw".into(),
            pgbackrest_password: "rpw".into(),
            preset: Preset::Tiny,
            pg_version: 18,
            host_port: 5544,
            backup_enabled: true,
            volume_name_override: None,
            retain_days: 30,
            snapshot_hour: Some(3),
            last_snapshot_at: Some("2026-05-13T03:00:00Z".into()),
            last_snapshot_attempt_at: Some("2026-05-13T03:00:00Z".into()),
            full_backup_day: 0,
        },
        created_at: "2026-05-01T00:00:00Z".into(),
    }
}

#[test]
fn restored_instance_has_archiving_and_scheduling_disabled() {
    // A restored instance is a recovery artifact. It restores from the
    // source's pgbackrest repo, so if it kept backup_enabled + a schedule it
    // would archive WAL on a new timeline straight back into the SOURCE's
    // stanza — corrupting the source's backup chain. It must be inert until
    // the operator explicitly re-enables backups.
    let restored = restored_instance_state(&source_state(), "rec1", 5599);
    assert!(
        !restored.instance.backup_enabled,
        "restored instance must not auto-archive into the source's stanza"
    );
    assert_eq!(
        restored.instance.snapshot_hour, None,
        "the scheduler must not touch a restored instance"
    );
    assert_eq!(restored.instance.last_snapshot_at, None);
    assert_eq!(restored.instance.last_snapshot_attempt_at, None);
}

#[test]
fn restored_instance_carries_identity_and_inherits_source_config() {
    let src = source_state();
    let restored = restored_instance_state(&src, "rec1", 5599);
    assert_eq!(restored.instance.name, "rec1");
    assert_eq!(restored.instance.host_port, 5599);
    assert_eq!(restored.instance.pg_version, src.instance.pg_version);
    assert_eq!(restored.instance.db_name, src.instance.db_name);
    assert_eq!(restored.instance.preset, src.instance.preset);
    assert_eq!(restored.instance.app_user, src.instance.app_user);
    assert_eq!(restored.instance.retain_days, src.instance.retain_days);
}
