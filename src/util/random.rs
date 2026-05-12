//! Auto-generated names + passwords for the TUI Create wizard.
//!
//! Both are alphanumeric (psql/JDBC URI-safe without %-encoding), random
//! via rand's thread-local CSPRNG. Length picks:
//! - password 24 chars ≈ 142 bits of entropy (24 × log2(62)). Plenty for
//!   a local PG role behind a docker port-mapped to 127.0.0.1.
//! - instance suffix 4 hex chars = 16 bits. The TUI guarantees it's not
//!   already used by suffixing-and-retrying up to 16 times before
//!   surrendering, so this just keeps the default name short.

use rand::Rng;
use rand::distributions::Alphanumeric;

pub fn password(n: usize) -> String {
    let mut rng = rand::thread_rng();
    (0..n).map(|_| rng.sample(Alphanumeric) as char).collect()
}

/// Suggested instance name like `pg-a3f1`. Lowercase, fits the
/// `[a-z][a-z0-9_-]{0,62}` regex pgforge enforces.
pub fn instance_name() -> String {
    let mut rng = rand::thread_rng();
    let hex: String = (0..4)
        .map(|_| {
            let v: u8 = rng.gen_range(0..16);
            if v < 10 { (b'0' + v) as char } else { (b'a' + v - 10) as char }
        })
        .collect();
    format!("pg-{hex}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_length_and_alphanumeric() {
        let pw = password(24);
        assert_eq!(pw.len(), 24);
        assert!(pw.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn password_distinct_each_call() {
        // Astronomically unlikely to collide; if this test ever flakes,
        // either RNG is broken or you should buy a lottery ticket.
        assert_ne!(password(24), password(24));
    }

    #[test]
    fn instance_name_matches_pgforge_regex() {
        for _ in 0..50 {
            let n = instance_name();
            assert!(n.starts_with("pg-"));
            assert_eq!(n.len(), 7); // "pg-" + 4 hex
            assert!(n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
        }
    }
}
