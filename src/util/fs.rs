use crate::error::{PgForgeError, Result};
use std::path::Path;

/// Write a file containing secret material (passwords, S3 keys, init SQL
/// embedding role passwords). Sets mode 0600 after write so other local users
/// cannot read it. On non-unix targets the chmod is a no-op and the file
/// inherits whatever permissions the OS gives — there is no analog of 0600
/// on plain Windows filesystems.
pub fn write_secret(path: &Path, content: impl AsRef<[u8]>) -> Result<()> {
    std::fs::write(path, content).map_err(|e| PgForgeError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(path, perms).map_err(|e| PgForgeError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
    }
    Ok(())
}

/// Create a directory that will hold secret files. Sets mode 0700 so other
/// local users cannot list/traverse the contents. Idempotent — succeeds if
/// the directory already exists.
pub fn create_secret_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).map_err(|e| PgForgeError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o700);
        std::fs::set_permissions(path, perms).map_err(|e| PgForgeError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
    }
    Ok(())
}
