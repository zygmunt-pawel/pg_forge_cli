use crate::error::{PgForgeError, Result};
use std::path::Path;

/// Write `content` to `path` atomically: write to `path.<pid>.tmp` in the same
/// directory, fsync the file, then rename over the destination. Creates the
/// parent directory if missing. Survives crash mid-write — readers either see
/// the previous content or the new content, never a truncated mix.
pub fn atomic_write(path: &Path, content: impl AsRef<[u8]>) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| PgForgeError::Io {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut f = std::fs::File::create(&tmp).map_err(|e| PgForgeError::Io {
        path: tmp.clone(),
        source: e,
    })?;
    f.write_all(content.as_ref()).map_err(|e| PgForgeError::Io {
        path: tmp.clone(),
        source: e,
    })?;
    f.sync_all().map_err(|e| PgForgeError::Io {
        path: tmp.clone(),
        source: e,
    })?;
    drop(f);
    std::fs::rename(&tmp, path).map_err(|e| PgForgeError::Io {
        path: path.to_path_buf(),
        source: e,
    })
}

/// Write a file containing secret material (passwords, S3 keys, init SQL
/// embedding role passwords). Sets mode 0600 after write so other local users
/// cannot read it. On non-unix targets the chmod is a no-op and the file
/// inherits whatever permissions the OS gives — there is no analog of 0600
/// on plain Windows filesystems.
pub fn write_secret(path: &Path, content: impl AsRef<[u8]>) -> Result<()> {
    atomic_write(path, content)?;
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
