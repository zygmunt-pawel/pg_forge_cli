use pgforge::docker::image::upgrade_dockerfile;
use pgforge::docker::upgrade_entrypoint::generate_upgrade_entrypoint;

#[test]
fn upgrade_dockerfile_installs_both_versions() {
    let df = upgrade_dockerfile(17, 18);
    assert!(df.contains("FROM postgres:18-bookworm"));
    assert!(df.contains("postgresql-17"));
    assert!(df.contains("pgbackrest"));
}

#[test]
fn upgrade_dockerfile_works_with_distant_versions() {
    let df = upgrade_dockerfile(13, 18);
    assert!(df.contains("FROM postgres:18-bookworm"));
    assert!(df.contains("postgresql-13"));
}

#[test]
fn upgrade_entrypoint_runs_initdb_then_pg_upgrade() {
    let s = generate_upgrade_entrypoint(17, 18);
    assert!(s.starts_with("#!/"));
    assert!(s.contains("set -eu"));
    // TO_BIN / FROM_BIN are shell vars in the entrypoint — assert their
    // values are wired correctly and that initdb / pg_upgrade reference them.
    assert!(s.contains("TO_BIN=/usr/lib/postgresql/18/bin"));
    assert!(s.contains("FROM_BIN=/usr/lib/postgresql/17/bin"));
    assert!(s.contains("$TO_BIN/initdb"));
    assert!(s.contains("$TO_BIN/pg_upgrade"));
    assert!(s.contains("--old-bindir=$FROM_BIN"));
    assert!(s.contains("--new-bindir=$TO_BIN"));
    assert!(s.contains("--old-datadir=$OLD_PGDATA"));
    assert!(s.contains("--new-datadir=$NEW_PGDATA"));
    assert!(s.contains("OLD_PGDATA=/old/pgdata"));
    assert!(s.contains("NEW_PGDATA=/new/pgdata"));
}

#[test]
fn upgrade_entrypoint_writes_success_marker_at_end() {
    // The caller relies on the marker file to verify pg_upgrade actually
    // completed (in addition to checking the exit code, which catches the
    // common case).
    let s = generate_upgrade_entrypoint(17, 18);
    assert!(s.contains(".pgforge-upgrade-complete"));
    // The touch must come AFTER the pg_upgrade invocation — checking it
    // appears later in the file is a coarse but useful guard against an
    // accidental reorder.
    let touch_pos = s.find("touch").expect("touch must appear");
    let upgrade_pos = s.find("pg_upgrade").expect("pg_upgrade must appear");
    assert!(upgrade_pos < touch_pos, "touch must follow pg_upgrade");
}

#[test]
fn upgrade_entrypoint_does_not_link_files() {
    // pg_upgrade's --link mode is faster but breaks rollback (old datadir
    // gets shared hard-links). We deliberately do a copy upgrade so the
    // old volume survives intact for `pgforge restore`.
    let s = generate_upgrade_entrypoint(17, 18);
    assert!(!s.contains("--link"), "must NOT use --link, breaks rollback");
}
