#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use herdr_quota::collector::{Collector, CollectorFailure};
use tempfile::TempDir;

const REPORT: &str =
    r#"{"generatedAt":"2026-09-02T12:00:00.000Z","schemaVersion":5,"providers":[]}"#;
const MAX_STDOUT_BYTES: usize = 2 * 1024 * 1024;

fn fake_executable(directory: &TempDir, name: &str, body: &str) -> PathBuf {
    let path = directory.path().join(name);
    fs::write(&path, format!("#!/usr/bin/env python3\n{body}\n"))
        .expect("fake executable must be written");
    make_executable(&path);
    path
}

fn fake_shell_executable(directory: &TempDir, name: &str, body: &str) -> PathBuf {
    let path = directory.path().join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("fake executable must be written");
    make_executable(&path);
    path
}

fn make_executable(path: &Path) {
    let mut permissions = fs::metadata(path)
        .expect("fake executable metadata must exist")
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(path, permissions).expect("fake executable must be executable");
}

fn collector(executable: &Path, directory: &TempDir, deadline: Duration) -> Collector {
    Collector::with_executable(executable, directory.path(), deadline)
}

fn current_result(
    collection: herdr_quota::collector::Collection,
) -> Result<herdr_quota::domain::schema::QuotaReport, CollectorFailure> {
    collection
        .publish(|result| result)
        .expect("collection must still be current")
}

fn python_string(path: &Path) -> String {
    format!("{:?}", path.to_string_lossy())
}

async fn wait_for_file(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "fake executable did not become ready"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn invokes_the_exact_schema_v5_command_in_the_selected_directory() {
    let directory = TempDir::new().expect("temporary directory must exist");
    let expected_directory = python_string(directory.path());
    let executable = fake_executable(
        &directory,
        "successful-quota-axi",
        &format!(
            r#"
import json
import os
import sys
assert sys.argv[1:] == ["--json", "--full", "--provider", "claude,codex,cursor,kimi,grok,copilot"]
assert os.path.samefile(os.getcwd(), {expected_directory})
assert os.environ["NO_COLOR"] == "1"
assert os.environ["TERM"] == "dumb"
sys.stdout.write({REPORT:?})
"#,
        ),
    );

    let report = current_result(
        collector(&executable, &directory, Duration::from_secs(3))
            .collect()
            .await,
    )
    .expect("valid schema-v5 output must collect");

    assert_eq!(report.schema_version, 5);
    assert!(report.providers.is_empty());
}

#[tokio::test]
async fn accepts_the_stdout_limit_and_rejects_one_byte_more() {
    let directory = TempDir::new().expect("temporary directory must exist");
    let executable = fake_executable(
        &directory,
        "bounded-quota-axi",
        &format!(
            r#"
import os
import sys
report = {REPORT:?}
size = {MAX_STDOUT_BYTES} + (1 if os.path.basename(sys.argv[0]).startswith("over") else 0)
sys.stdout.write(report)
sys.stdout.write(" " * (size - len(report)))
"#,
        ),
    );
    let at_limit = current_result(
        collector(&executable, &directory, Duration::from_secs(3))
            .collect()
            .await,
    );
    assert!(at_limit.is_ok());

    let over_executable = directory.path().join("over-quota-axi");
    fs::copy(&executable, &over_executable).expect("over-limit fake must be copied");
    let over_limit = current_result(
        collector(&over_executable, &directory, Duration::from_secs(3))
            .collect()
            .await,
    );
    assert_eq!(over_limit, Err(CollectorFailure::IncompatibleOutput));
}

#[tokio::test]
async fn timeout_sends_sigterm_and_returns_only_the_safe_failure() {
    let directory = TempDir::new().expect("temporary directory must exist");
    let terminated = directory.path().join("terminated");
    let executable = fake_shell_executable(
        &directory,
        "slow-quota-axi",
        &format!(
            r#"
trap 'touch {}; exit 0' TERM
while :; do sleep 0.1; done
"#,
            terminated.to_string_lossy()
        ),
    );

    let started = Instant::now();
    let failure = current_result(
        collector(&executable, &directory, Duration::from_secs(2))
            .collect()
            .await,
    )
    .expect_err("stalled process must time out");

    assert_eq!(failure, CollectorFailure::Timeout);
    assert!(terminated.exists());
    assert!(started.elapsed() >= Duration::from_millis(1_900));
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(failure.to_string(), "Quota check timed out");
}

#[tokio::test]
async fn timeout_escalates_after_500ms_and_reaps_a_term_ignoring_child() {
    let directory = TempDir::new().expect("temporary directory must exist");
    let pid_file = directory.path().join("pid");
    let executable = fake_shell_executable(
        &directory,
        "term-ignoring-quota-axi",
        &format!(
            r#"
trap '' TERM
echo $$ > {}
exec tail -f /dev/null
"#,
            pid_file.to_string_lossy()
        ),
    );

    let started = Instant::now();
    let failure = current_result(
        collector(&executable, &directory, Duration::from_secs(2))
            .collect()
            .await,
    )
    .expect_err("term-ignoring process must time out");
    let elapsed = started.elapsed();
    let process_id = fs::read_to_string(&pid_file)
        .expect("fake process must record its identifier")
        .trim()
        .parse::<libc::pid_t>()
        .expect("recorded identifier must be numeric");

    assert_eq!(failure, CollectorFailure::Timeout);
    assert!(elapsed >= Duration::from_millis(2_450));
    assert!(elapsed < Duration::from_secs(4));
    // SAFETY: signal 0 only checks whether the already-reaped test PID exists.
    assert_eq!(unsafe { libc::kill(process_id, 0) }, -1);
}

#[tokio::test]
async fn cancellation_and_new_generations_suppress_late_publication() {
    let directory = TempDir::new().expect("temporary directory must exist");
    let first = directory.path().join("first");
    let ready = directory.path().join("ready");
    let executable = fake_executable(
        &directory,
        "racing-quota-axi",
        &format!(
            r#"
import os
import signal
import sys
import time
try:
    descriptor = os.open({}, os.O_CREAT | os.O_EXCL | os.O_WRONLY)
except FileExistsError:
    sys.stdout.write({REPORT:?})
    sys.exit(0)
os.close(descriptor)
signal.signal(signal.SIGTERM, signal.SIG_IGN)
open({}, "w").close()
time.sleep(0.2)
sys.stdout.write({REPORT:?})
"#,
            python_string(&first),
            python_string(&ready),
        ),
    );
    let collector = collector(&executable, &directory, Duration::from_secs(2));
    let first_collector = collector.clone();
    let first_attempt = tokio::spawn(async move { first_collector.collect().await });
    wait_for_file(&ready).await;

    let second = collector.collect().await;
    let second_report = current_result(second).expect("new generation must succeed");
    let late_first = first_attempt
        .await
        .expect("first collection task must finish");

    assert_eq!(second_report.schema_version, 5);
    assert!(late_first.publish(|_| ()).is_none());
}

#[tokio::test]
async fn explicit_cancellation_terminates_the_active_child_without_publication() {
    let directory = TempDir::new().expect("temporary directory must exist");
    let ready = directory.path().join("ready");
    let terminated = directory.path().join("terminated");
    let executable = fake_executable(
        &directory,
        "cancelled-quota-axi",
        &format!(
            r#"
import signal
import sys
import time
def terminate(_signal, _frame):
    open({}, "w").close()
    sys.exit(0)
signal.signal(signal.SIGTERM, terminate)
open({}, "w").close()
while True:
    time.sleep(1)
"#,
            python_string(&terminated),
            python_string(&ready),
        ),
    );
    let collector = collector(&executable, &directory, Duration::from_secs(2));
    let active_collector = collector.clone();
    let attempt = tokio::spawn(async move { active_collector.collect().await });
    wait_for_file(&ready).await;

    collector.cancel();
    let cancelled = attempt
        .await
        .expect("cancelled collection task must finish");

    assert!(terminated.exists());
    assert!(cancelled.publish(|_| ()).is_none());
}

#[tokio::test]
async fn process_schema_and_stderr_details_never_enter_public_failures() {
    let directory = TempDir::new().expect("temporary directory must exist");
    let missing = directory.path().join("missing-quota-axi");
    let missing_failure = current_result(
        collector(&missing, &directory, Duration::from_secs(5))
            .collect()
            .await,
    )
    .expect_err("missing executable must fail");
    assert_eq!(missing_failure, CollectorFailure::MissingExecutable);

    let failing = fake_shell_executable(
        &directory,
        "failing-quota-axi",
        r#"
printf '\033[2JBearer secret.token.value from /home/alice/auth.json api-key-abcdefghijk' >&2
exit 2
"#,
    );
    let process_failure = current_result(
        collector(&failing, &directory, Duration::from_secs(5))
            .collect()
            .await,
    )
    .expect_err("nonzero process must fail");
    assert_eq!(process_failure, CollectorFailure::NetworkProcess);

    let incompatible = fake_shell_executable(
        &directory,
        "incompatible-quota-axi",
        r#"
printf '%s' '{"schemaVersion":6,"raw":"Bearer secret.token.value /home/alice/auth.json"}'
"#,
    );
    let schema_failure = current_result(
        collector(&incompatible, &directory, Duration::from_secs(5))
            .collect()
            .await,
    )
    .expect_err("unsupported schema must fail");
    assert_eq!(schema_failure, CollectorFailure::IncompatibleOutput);

    for failure in [missing_failure, process_failure, schema_failure] {
        let public = format!("{failure:?} {failure}");
        assert!(!public.contains("secret"));
        assert!(!public.contains("alice"));
        assert!(!public.contains("auth.json"));
        assert!(!public.contains("api-key"));
        assert!(!public.contains('\u{1b}'));
    }
}
