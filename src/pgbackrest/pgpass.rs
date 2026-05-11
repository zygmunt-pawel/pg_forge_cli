/// Render a `.pgpass` file allowing the `pgbackrest` role to authenticate
/// without prompting. Format per Postgres docs:
///   hostname:port:database:username:password
/// `\` and `:` in the password field must be escaped with `\`.
pub fn generate_pgpass(pgbackrest_password: &str) -> String {
    let escaped: String = pgbackrest_password
        .chars()
        .flat_map(|c| match c {
            '\\' => vec!['\\', '\\'],
            ':' => vec!['\\', ':'],
            other => vec![other],
        })
        .collect();
    format!("*:*:*:pgbackrest:{escaped}\n")
}
