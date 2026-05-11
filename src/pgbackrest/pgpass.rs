/// Render a `.pgpass` file allowing the `pgreplica` role to authenticate
/// without prompting (used by `pgforge clone`'s pg_basebackup over TCP).
/// Format per Postgres docs:
///   hostname:port:database:username:password
/// `\` and `:` in the password field must be escaped with `\`.
pub fn generate_pgpass(replication_password: &str) -> String {
    let escaped: String = replication_password
        .chars()
        .flat_map(|c| match c {
            '\\' => vec!['\\', '\\'],
            ':' => vec!['\\', ':'],
            other => vec![other],
        })
        .collect();
    format!("*:*:*:pgreplica:{escaped}\n")
}
