/// Render the Dockerfile that bakes pgbackrest + cron on top of the official
/// postgres image. Pure function — output is deterministic per pg_version.
pub fn dockerfile(pg_version: u8) -> String {
    format!(
        r#"FROM postgres:{ver}-bookworm

ENV DEBIAN_FRONTEND=noninteractive

RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends \
        pgbackrest cron tini ca-certificates tzdata; \
    rm -rf /var/lib/apt/lists/*; \
    mkdir -p /var/spool/pgbackrest /var/log/pgbackrest /etc/pgbackrest; \
    chown -R postgres:postgres /var/spool/pgbackrest /var/log/pgbackrest /etc/pgbackrest

# Tag of the image is set at build time by pgforge: pgforge/postgres:{ver}
"#,
        ver = pg_version
    )
}
