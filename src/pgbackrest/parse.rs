/// Extract the "new backup label = <LABEL>" line from `pgbackrest backup`
/// stdout. Returns None if the line isn't present (e.g. backup failed
/// before the label was assigned).
pub fn parse_backup_label(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        if let Some(idx) = line.find("new backup label =") {
            let rest = &line[idx + "new backup label =".len()..];
            let label = rest.trim();
            if !label.is_empty() {
                return Some(label.to_string());
            }
        }
    }
    None
}
