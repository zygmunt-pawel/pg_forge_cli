//! `pgforge self-update` — pull the latest binary from this repo's
//! GitHub Releases and atomically replace the running binary.
//!
//! Strategy is intentionally simple: shell out to `curl` (always
//! present on macOS / Linux) so we don't add a heavy HTTP/TLS
//! dependency just for this rarely-used path. Two curl calls:
//!  1. GitHub API for the latest release tag.
//!  2. Direct download of the universal macOS binary asset.
//!
//! Atomic replacement = write to `<current_exe>.new` on the same
//! filesystem, chmod +x, then `rename` it over the live binary. On
//! Unix this is safe even while the old binary is currently executing:
//! the running process keeps its own inode mapped; the next exec
//! picks up the new file.

use crate::error::{PgForgeError, Result};
use std::path::{Path, PathBuf};

const GITHUB_REPO: &str = "zygmunt-pawel/pg_forge_cli";

#[derive(Debug, Clone)]
pub struct SelfUpdateOutcome {
    /// `vX.Y.Z` from the GitHub release. Always set even when we
    /// noticed the user is already on this version.
    pub latest_tag: String,
    /// True when we actually downloaded + replaced the binary.
    pub upgraded: bool,
    /// What the running process reported as its version before the
    /// check, formatted as `vX.Y.Z` to match GitHub tag style.
    pub current_version: String,
}

pub async fn run(force: bool) -> Result<SelfUpdateOutcome> {
    let current = format!("v{}", env!("CARGO_PKG_VERSION"));
    let latest = fetch_latest_tag().await?;
    if latest == current && !force {
        return Ok(SelfUpdateOutcome {
            latest_tag: latest,
            upgraded: false,
            current_version: current,
        });
    }

    let exe = std::env::current_exe().map_err(|e| {
        PgForgeError::Anyhow(anyhow::anyhow!("current_exe: {e}"))
    })?;
    // Resolve symlinks (Homebrew etc.) so we replace the real file,
    // not a symlink leftover that points elsewhere.
    let exe = exe.canonicalize().unwrap_or(exe);
    let tmp = sibling_with_suffix(&exe, ".new");

    let url = format!(
        "https://github.com/{repo}/releases/download/{tag}/pgforge",
        repo = GITHUB_REPO,
        tag = latest,
    );
    download_to(&url, &tmp).await?;
    set_executable(&tmp)?;
    std::fs::rename(&tmp, &exe).map_err(|e| PgForgeError::Io {
        path: exe.clone(),
        source: e,
    })?;

    Ok(SelfUpdateOutcome {
        latest_tag: latest,
        upgraded: true,
        current_version: current,
    })
}

fn sibling_with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

async fn fetch_latest_tag() -> Result<String> {
    let url = format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest");
    let out = tokio::process::Command::new("curl")
        .args(["-sSfL", "-H", "Accept: application/vnd.github+json"])
        // GitHub API requires a User-Agent header.
        .args(["-H", "User-Agent: pgforge-self-update"])
        .arg(&url)
        .output()
        .await
        .map_err(|e| PgForgeError::Anyhow(anyhow::anyhow!("spawn curl: {e}")))?;
    if !out.status.success() {
        return Err(PgForgeError::Anyhow(anyhow::anyhow!(
            "github api: curl exit {} stderr={}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| {
        PgForgeError::Anyhow(anyhow::anyhow!("parse github json: {e}"))
    })?;
    v.get("tag_name")
        .and_then(|s| s.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            PgForgeError::Anyhow(anyhow::anyhow!(
                "github releases/latest: no tag_name in response"
            ))
        })
}

async fn download_to(url: &str, dest: &Path) -> Result<()> {
    let out = tokio::process::Command::new("curl")
        .args(["-sSfL", "-o"])
        .arg(dest)
        .arg(url)
        .output()
        .await
        .map_err(|e| PgForgeError::Anyhow(anyhow::anyhow!("spawn curl download: {e}")))?;
    if !out.status.success() {
        // Clean up the half-downloaded file so a retry has a clean slate.
        let _ = std::fs::remove_file(dest);
        return Err(PgForgeError::Anyhow(anyhow::anyhow!(
            "download {url}: curl exit {} stderr={}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn set_executable(p: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(p)
        .map_err(|e| PgForgeError::Io { path: p.to_path_buf(), source: e })?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(p, perms).map_err(|e| PgForgeError::Io {
        path: p.to_path_buf(),
        source: e,
    })
}

#[cfg(not(unix))]
fn set_executable(_p: &Path) -> Result<()> { Ok(()) }
