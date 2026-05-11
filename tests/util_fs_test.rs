use pgforge::util::fs::{create_secret_dir, write_secret};

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
