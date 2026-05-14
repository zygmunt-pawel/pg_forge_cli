use pgforge::util::fs::{atomic_write, create_secret_dir, fsync_dir, write_secret};
use pgforge::util::fs::LockedStateRoot;

#[test]
fn fsync_dir_succeeds_on_existing_directory() {
    let dir = tempfile::tempdir().unwrap();
    fsync_dir(dir.path()).unwrap();
}

#[test]
fn fsync_dir_errors_on_missing_directory() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("does-not-exist");
    assert!(
        fsync_dir(&missing).is_err(),
        "fsync_dir must surface an error for a non-existent directory"
    );
}

#[test]
fn write_secret_sets_mode_0600_on_unix() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("secret.toml");
    write_secret(&path, "password = \"hunter2\"").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "password = \"hunter2\"");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0600, got {mode:o}");
    }
}

#[test]
fn create_secret_dir_sets_mode_0700_on_unix() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("conf");
    create_secret_dir(&path).unwrap();
    assert!(path.is_dir());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "expected 0700, got {mode:o}");
    }
}

#[test]
fn create_secret_dir_is_idempotent() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("conf");
    create_secret_dir(&path).unwrap();
    // Calling again on an existing dir should succeed.
    create_secret_dir(&path).unwrap();
}

#[test]
fn write_secret_overwrites_existing_file_and_resets_mode() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("secret");
    std::fs::write(&path, "old").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o644);
        std::fs::set_permissions(&path, perms).unwrap();
    }
    write_secret(&path, "new").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "must reset mode to 0600 on overwrite, got {mode:o}");
    }
}

#[test]
fn atomic_write_creates_file_with_content() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("sub").join("a.toml");
    atomic_write(&p, b"hello").unwrap();
    assert_eq!(std::fs::read(&p).unwrap(), b"hello");
}

#[test]
fn atomic_write_overwrites_existing() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.toml");
    std::fs::write(&p, b"old").unwrap();
    atomic_write(&p, b"new").unwrap();
    assert_eq!(std::fs::read(&p).unwrap(), b"new");
}

#[test]
fn atomic_write_leaves_no_tmp_on_success() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.toml");
    atomic_write(&p, b"x").unwrap();
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name() != std::ffi::OsString::from("a.toml"))
        .collect();
    assert!(leftovers.is_empty(), "found: {:?}", leftovers);
}

#[test]
fn locked_state_root_is_exclusive() {
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let o1 = order.clone();
    let r1 = root.clone();
    let t1 = thread::spawn(move || {
        let _g = LockedStateRoot::acquire(&r1).unwrap();
        o1.lock().unwrap().push("t1-acquired");
        thread::sleep(Duration::from_millis(200));
        o1.lock().unwrap().push("t1-released");
    });
    thread::sleep(Duration::from_millis(50));
    let o2 = order.clone();
    let r2 = root.clone();
    let t2 = thread::spawn(move || {
        let _g = LockedStateRoot::acquire(&r2).unwrap();
        o2.lock().unwrap().push("t2-acquired");
    });
    t1.join().unwrap();
    t2.join().unwrap();
    let o = order.lock().unwrap().clone();
    assert_eq!(o, vec!["t1-acquired", "t1-released", "t2-acquired"]);
}
