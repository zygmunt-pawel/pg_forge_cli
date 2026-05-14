use async_trait::async_trait;
use pgforge::docker::engine::{
    BuildImageSpec, ContainerInspect, CreateContainerSpec, DockerEngine, ExecOutput,
};
use pgforge::docker::wait::wait_for_recovery_end;
use pgforge::error::Result;
use std::sync::Mutex;
use std::time::Duration;

/// Records every `exec` it receives; returns "f" (not in recovery) so
/// `wait_for_recovery_end` resolves on the first poll. Every other trait
/// method is unused by `wait_for_recovery_end`.
#[derive(Default)]
struct RecordingEngine {
    exec_calls: Mutex<Vec<Vec<String>>>,
}

#[async_trait]
impl DockerEngine for RecordingEngine {
    async fn exec(&self, _id: &str, cmd: &[&str]) -> Result<ExecOutput> {
        self.exec_calls
            .lock()
            .unwrap()
            .push(cmd.iter().map(|s| s.to_string()).collect());
        Ok(ExecOutput {
            stdout: "f".into(),
            stderr: String::new(),
            exit_code: 0,
        })
    }
    async fn build_image(&self, _: &BuildImageSpec) -> Result<()> { unimplemented!() }
    async fn ensure_network(&self, _: &str) -> Result<()> { unimplemented!() }
    async fn create_container(&self, _: &CreateContainerSpec) -> Result<String> { unimplemented!() }
    async fn start_container(&self, _: &str) -> Result<()> { unimplemented!() }
    async fn container_exists(&self, _: &str) -> Result<bool> { unimplemented!() }
    async fn container_running(&self, _: &str) -> Result<bool> { unimplemented!() }
    async fn exec_as(&self, _: &str, _: &str, _: &[&str]) -> Result<ExecOutput> { unimplemented!() }
    async fn exec_with_stdin(&self, _: &str, _: &[&str], _: &str) -> Result<ExecOutput> {
        unimplemented!()
    }
    async fn stop_container(&self, _: &str) -> Result<()> { unimplemented!() }
    async fn wait_for_container_running(&self, _: &str, _: Duration) -> Result<()> {
        unimplemented!()
    }
    async fn wait_for_container_exit(&self, _: &str, _: Duration) -> Result<i64> { unimplemented!() }
    async fn remove_container(&self, _: &str, _: bool) -> Result<()> { unimplemented!() }
    async fn remove_volume(&self, _: &str) -> Result<()> { unimplemented!() }
    async fn inspect_container(&self, _: &str) -> Result<ContainerInspect> { unimplemented!() }
    async fn logs(&self, _: &str) -> Result<String> { unimplemented!() }
}

#[tokio::test]
async fn wait_for_recovery_end_connects_as_a_role_pg_hba_permits() {
    let eng = RecordingEngine::default();
    wait_for_recovery_end(&eng, "cid", 5).await.unwrap();
    let calls = eng.exec_calls.lock().unwrap();
    let cmd = calls.first().expect("should have issued at least one exec");
    // pgforge's generated pg_hba.conf has NO `local` line for the `postgres`
    // role — only the app_user and `pgbackrest` are trusted on the socket.
    // Connecting as `postgres` gets "no pg_hba.conf entry" and the wait burns
    // its whole deadline against a healthy, already-promoted postgres.
    assert!(
        cmd.contains(&"pgbackrest".to_string()),
        "must connect as a role pg_hba permits (pgbackrest), got: {cmd:?}"
    );
    assert!(
        !cmd.windows(2).any(|w| w == ["-U", "postgres"]),
        "must NOT connect as `-U postgres` (no pg_hba entry), got: {cmd:?}"
    );
}

#[tokio::test]
async fn wait_for_recovery_end_query_is_consistent_with_its_parser() {
    // The parser accepts only `stdout.trim() == "f"`. `psql -tA` renders a
    // raw boolean as `f`/`t`, but `pg_is_in_recovery()::text` renders as
    // `false`/`true` — so a `::text` cast makes the check never match and
    // the wait burns its whole deadline against an already-promoted postgres.
    let eng = RecordingEngine::default();
    wait_for_recovery_end(&eng, "cid", 5).await.unwrap();
    let calls = eng.exec_calls.lock().unwrap();
    let cmd = calls.first().unwrap().join(" ");
    assert!(
        cmd.contains("pg_is_in_recovery()"),
        "must query pg_is_in_recovery(), got: {cmd}"
    );
    assert!(
        !cmd.contains("::text"),
        "must NOT cast to ::text — psql -tA renders a bool as f/t and the \
         parser checks == \"f\"; ::text renders false/true and never matches: {cmd}"
    );
}
