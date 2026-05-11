use crate::error::{PgForgeError, Result};
use jiff::Timestamp;

/// Current UTC instant rendered as `YYYY-MM-DDTHH:MM:SSZ` (exactly 20 chars).
pub fn now_iso() -> String {
    let now = Timestamp::now();
    // Truncate to second precision so output is deterministic length.
    let secs = now.as_second();
    Timestamp::from_second(secs)
        .map(|t| t.to_string())
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into())
}

/// Parse a user-supplied target time. Accepts:
/// - Full RFC 3339:           `2026-05-10T14:23:00Z`
/// - RFC 3339 with offset:    `2026-05-10T14:23:00+02:00`
/// - Space separator variant: `2026-05-10 14:23:00` (assumed UTC)
pub fn parse_target_time(s: &str) -> Result<Timestamp> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(PgForgeError::Anyhow(anyhow::anyhow!("empty target-time")));
    }
    // Try full RFC 3339 first
    if let Ok(t) = trimmed.parse::<Timestamp>() {
        return Ok(t);
    }
    // Then try the space-separator variant by replacing the space with `T`
    // and assuming UTC (Z suffix).
    if let Some((d, t)) = trimmed.split_once(' ') {
        let candidate = format!("{d}T{t}Z");
        if let Ok(t) = candidate.parse::<Timestamp>() {
            return Ok(t);
        }
    }
    Err(PgForgeError::Anyhow(anyhow::anyhow!(
        "cannot parse target-time {trimmed:?} — expected RFC 3339 (e.g. 2026-05-10T14:23:00Z)"
    )))
}
