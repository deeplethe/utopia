use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::str::FromStr;
use std::sync::{mpsc, OnceLock};
use std::time::Duration;

use sha2::{Digest, Sha256};
use sqlx::{postgres::PgConnectOptions, ConnectOptions, Connection, Executor, PgConnection};

fn pg_restore_stub() -> PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        // 共享替身放在构建目录，由 cargo clean 统一清理。
        let binary = std::env::current_exe().unwrap().with_file_name(format!(
            "utopia-pg-restore-test-{}{}",
            std::process::id(),
            std::env::consts::EXE_SUFFIX
        ));
        let status = Command::new("rustc")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/pg_restore.rs"))
            .arg("-o")
            .arg(&binary)
            .status()
            .unwrap();
        assert!(status.success(), "compile the recording pg_restore double");
        binary
    })
    .clone()
}

struct RestoreTest {
    root: tempfile::TempDir,
    admin_url: String,
    database: String,
    url: String,
}

impl RestoreTest {
    async fn new(with_data: bool) -> Option<Self> {
        let admin_url = utopia_store::test_db::url()?;
        let root = tempfile::tempdir().unwrap();
        let database = format!(
            "utopia_restore_{}_{}",
            std::process::id(),
            root.path()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .replace('.', "")
        )
        .to_lowercase();
        let options = PgConnectOptions::from_str(&admin_url).unwrap();
        let mut admin = PgConnection::connect_with(&options).await.unwrap();
        // 只修改和删除本例新建的数据库，不触碰 guard 提供的数据库。
        admin
            .execute(format!("CREATE DATABASE {database}").as_str())
            .await
            .unwrap();
        let mut target_url = options.database(&database).to_url_lossy();
        // SQLx 自动追加的客户端参数不被 libpq 的 pg_dump/pg_restore 接受。
        let parameters: Vec<_> = target_url
            .query_pairs()
            .filter(|(name, _)| name != "statement-cache-capacity")
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect();
        target_url
            .query_pairs_mut()
            .clear()
            .extend_pairs(parameters);
        let fixture = Self {
            root,
            admin_url,
            url: target_url.to_string(),
            database,
        };
        std::fs::create_dir(fixture.source()).unwrap();
        std::fs::write(fixture.source().join("pg_dump.custom"), b"test dump").unwrap();
        if with_data {
            std::fs::create_dir_all(fixture.source().join("data/files")).unwrap();
            std::fs::write(fixture.source().join("data/files/new.bin"), b"new data").unwrap();
            std::fs::write(fixture.source().join("data/secret.key"), b"archive key").unwrap();
        }
        let version =
            std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations"))
                .unwrap()
                .map(|entry| entry.expect("read migration directory entry"))
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "sql"))
                .count();
        let digest: String = Sha256::digest(b"test dump")
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let checksum = format!("sha256:{digest}");
        let manifest = serde_json::json!({
            "schema_version": version, "utopia_version": "0.1.0", "created_at": "2026-10-25T00:00:00Z",
            "components": {
                "pg_dump": {"path": "pg_dump.custom", "format": "pg_dump -Fc", "bytes": 9},
                "data_dir": {"path": "data", "present": with_data, "secret_key": with_data}
            },
            "checksums": {"pg_dump.custom": checksum}
        });
        std::fs::write(
            fixture.source().join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        fixture.pack();
        Some(fixture)
    }

    fn source(&self) -> PathBuf {
        self.root.path().join("source")
    }
    fn target(&self) -> PathBuf {
        self.root.path().join("target")
    }
    fn log(&self) -> PathBuf {
        self.root.path().join("restore.log")
    }
    fn pack(&self) {
        let status = Command::new("tar")
            .arg("-czf")
            .arg(self.root.path().join("backup.tar.gz"))
            .arg("-C")
            .arg(self.source())
            .arg(".")
            .status()
            .unwrap();
        assert!(status.success());
    }

    fn edit_manifest(&self, edit: impl FnOnce(&mut serde_json::Value)) {
        let path = self.source().join("manifest.json");
        let mut manifest = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        edit(&mut manifest);
        std::fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    }

    fn cli(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_utopia"));
        command
            .current_dir(self.root.path())
            .env("UTOPIA_DATABASE_URL", &self.url)
            .env("UTOPIA_MIGRATION_URL", &self.url)
            .env("UTOPIA_DATA_DIR", self.source().join("data"))
            .stdin(Stdio::null());
        command
    }

    fn command(&self, flags: &[&str]) -> Command {
        let mut command = self.cli();
        command
            .args(["restore", "--from"])
            .arg(self.root.path().join("backup.tar.gz"))
            .arg("--target-data-dir")
            .arg(self.target())
            .arg("--pg-restore")
            .arg(pg_restore_stub())
            .args(flags)
            .env("UTOPIA_RESTORE_TEST_LOG", self.log());
        command
    }

    fn restore(&self, flags: &[&str]) -> Output {
        self.command(flags).output().unwrap()
    }
    fn awaiting_confirmation(&self) -> Child {
        let mut child = self
            .command(&[])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let (sender, receiver) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut prompt = Vec::new();
            let result = loop {
                let mut byte = [0];
                if let Err(error) = stderr.read_exact(&mut byte) {
                    break Err(error);
                }
                prompt.push(byte[0]);
                if prompt.ends_with(b"Continue? [y/N] ") {
                    break Ok(());
                }
            };
            // 超时后接收端会关闭，此时发送失败属于正常清理。
            let _ = sender.send((result, stderr, prompt));
        });
        let error = match receiver.recv_timeout(Duration::from_secs(30)) {
            Ok((Ok(()), stderr, _)) => {
                reader.join().unwrap();
                child.stderr = Some(stderr);
                return child;
            }
            Ok((Err(error), _, prompt)) => {
                format!("{error}; stderr: {}", String::from_utf8_lossy(&prompt))
            }
            Err(error) => format!("confirmation wait failed: {error}"),
        };
        drop(receiver);
        let _ = child.kill();
        let status = child
            .wait()
            .expect("reap restore after confirmation failure");
        reader.join().unwrap();
        panic!("restore did not reach confirmation: {error}; child status: {status}");
    }
    fn calls(&self) -> String {
        let log = self.log();
        match std::fs::read_to_string(&log) {
            Ok(calls) => calls,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => panic!("cannot read pg_restore call log {}: {error}", log.display()),
        }
    }
    fn old_data(&self) {
        std::fs::create_dir_all(self.target().join("files")).unwrap();
        std::fs::write(self.target().join("files/old.bin"), b"old data").unwrap();
        std::fs::write(self.target().join("secret.key"), b"old key").unwrap();
    }
    fn assert_old_data(&self) {
        assert_eq!(
            std::fs::read(self.target().join("files/old.bin")).unwrap(),
            b"old data"
        );
        assert_eq!(
            std::fs::read(self.target().join("secret.key")).unwrap(),
            b"old key"
        );
        assert!(!self.target().join("files/new.bin").exists());
    }
    async fn connection(&self) -> PgConnection {
        PgConnection::connect(&self.url).await.unwrap()
    }
}

impl Drop for RestoreTest {
    fn drop(&mut self) {
        let admin_url = self.admin_url.clone();
        let database = self.database.clone();
        let cleanup = std::thread::spawn(move || {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                let mut admin = PgConnection::connect(&admin_url).await.unwrap();
                admin
                    .execute(format!("DROP DATABASE {database} WITH (FORCE)").as_str())
                    .await
                    .unwrap();
            });
        })
        .join();
        if let Err(panic_payload) = cleanup {
            eprintln!("failed to clean up test database {}", self.database);
            // 清理失败时保留最初的测试错误，避免 Drop 中二次 panic 中止整个进程。
            if !std::thread::panicking() {
                std::panic::resume_unwind(panic_payload);
            }
        }
    }
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_failure(output: &Output, message: &str) {
    assert!(
        !output.status.success(),
        "unexpected success: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(message),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn nonempty_directory_requires_force_before_database_restore() {
    let Some(fixture) = RestoreTest::new(true).await else {
        return;
    };
    fixture.old_data();
    for flags in [vec!["--yes"], vec!["--yes", "--dry-run"]] {
        assert_failure(&fixture.restore(&flags), "--force");
        assert_eq!(fixture.calls(), "", "preflight must not call pg_restore");
        fixture.assert_old_data();
    }
}

#[tokio::test]
async fn database_changed_during_confirmation_is_rejected_before_restore() {
    let Some(fixture) = RestoreTest::new(false).await else {
        return;
    };
    let mut child = fixture.awaiting_confirmation();
    let mut connection = fixture.connection().await;
    connection
        .execute(
            "CREATE TABLE confirmation_sentinel (value integer);
             INSERT INTO confirmation_sentinel VALUES (99)",
        )
        .await
        .unwrap();
    child.stdin.take().unwrap().write_all(b"y\n").unwrap();
    assert_failure(
        &child.wait_with_output().unwrap(),
        "target database is non-empty",
    );
    assert_eq!(
        fixture.calls(),
        "",
        "confirmation must not bypass the database overwrite guard"
    );
    let unchanged: i32 = sqlx::query_scalar("SELECT value FROM confirmation_sentinel")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(unchanged, 99);
    assert!(!fixture.target().exists());
}

#[tokio::test]
async fn data_directory_changed_during_confirmation_is_rejected_before_restore() {
    let Some(fixture) = RestoreTest::new(true).await else {
        return;
    };
    let mut child = fixture.awaiting_confirmation();
    fixture.old_data();
    child.stdin.take().unwrap().write_all(b"y\n").unwrap();
    assert_failure(&child.wait_with_output().unwrap(), "--force");
    assert_eq!(
        fixture.calls(),
        "",
        "confirmation must not bypass the directory overwrite guard"
    );
    fixture.assert_old_data();
}

#[tokio::test]
async fn force_replaces_old_files_and_key_instead_of_merging() {
    let Some(fixture) = RestoreTest::new(true).await else {
        return;
    };
    fixture.old_data();
    assert_success(&fixture.restore(&["--yes", "--force", "--dry-run"]));
    fixture.assert_old_data();
    assert_eq!(fixture.calls(), "");
    let output = fixture
        .command(&["--yes", "--force"])
        .env(
            "UTOPIA_RESTORE_TEST_OLD_FILE",
            fixture.target().join("files/old.bin"),
        )
        .output()
        .unwrap();
    assert_success(&output);
    let calls = fixture.calls();
    assert_eq!(calls.lines().count(), 1);
    for flag in [
        "--clean",
        "--if-exists",
        "--exit-on-error",
        "--single-transaction",
    ] {
        assert!(calls.contains(flag), "{calls}");
    }
    assert!(!fixture.target().join("files/old.bin").exists());
    assert_eq!(
        std::fs::read(fixture.target().join("files/new.bin")).unwrap(),
        b"new data"
    );
    assert_eq!(
        std::fs::read(fixture.target().join("secret.key")).unwrap(),
        b"archive key"
    );
}

#[tokio::test]
async fn empty_tables_and_other_user_objects_also_require_force() {
    for statement in [
        "CREATE TABLE restore_empty (value integer)",
        "CREATE SCHEMA restore_schema",
        "CREATE FUNCTION restore_function() RETURNS integer LANGUAGE sql AS 'SELECT 1'",
        "CREATE TYPE restore_type AS ENUM ('value')",
        "CREATE VIEW restore_view AS SELECT 1 AS value",
        "CREATE PUBLICATION restore_publication",
        "CREATE FOREIGN DATA WRAPPER restore_fdw",
        "SELECT pg_catalog.lo_create(0)",
    ] {
        let Some(fixture) = RestoreTest::new(false).await else {
            return;
        };
        fixture.connection().await.execute(statement).await.unwrap();
        for flags in [vec!["--yes"], vec!["--yes", "--dry-run"]] {
            assert_failure(&fixture.restore(&flags), "--force");
            assert_eq!(
                fixture.calls(),
                "",
                "{statement} bypassed overwrite protection"
            );
        }
    }
}

#[tokio::test]
async fn missing_components_and_invalid_targets_fail_before_restore() {
    for case in [
        "missing-data",
        "missing-dump",
        "dump-dir",
        "data-file",
        "target-file",
        "parent-file",
    ] {
        let Some(fixture) = RestoreTest::new(true).await else {
            return;
        };
        match case {
            "missing-data" => std::fs::remove_dir_all(fixture.source().join("data")).unwrap(),
            "missing-dump" | "dump-dir" => {
                std::fs::remove_file(fixture.source().join("pg_dump.custom")).unwrap();
                if case == "dump-dir" {
                    std::fs::create_dir(fixture.source().join("pg_dump.custom")).unwrap();
                }
                fixture.edit_manifest(|manifest| manifest["checksums"] = serde_json::json!({}));
            }
            "data-file" => {
                std::fs::remove_dir_all(fixture.source().join("data")).unwrap();
                std::fs::write(fixture.source().join("data"), b"not a directory").unwrap();
            }
            "target-file" => std::fs::write(fixture.target(), b"keep this file").unwrap(),
            "parent-file" => {
                std::fs::write(fixture.target(), b"keep this parent").unwrap();
            }
            _ => unreachable!(),
        }
        fixture.pack();
        for dry_run in [false, true] {
            let mut command = fixture.command(&["--yes", "--force"]);
            if case == "parent-file" {
                command
                    .arg("--target-data-dir")
                    .arg(fixture.target().join("nested"));
            }
            if dry_run {
                command.arg("--dry-run");
            }
            let output = command.output().unwrap();
            assert!(
                !output.status.success(),
                "{case} accepted (dry_run={dry_run})"
            );
            assert_eq!(
                fixture.calls(),
                "",
                "{case} ran pg_restore (dry_run={dry_run})"
            );
            if fixture.target().is_file() {
                assert!(std::fs::read(fixture.target())
                    .unwrap()
                    .starts_with(b"keep this"));
            } else {
                assert!(!fixture.target().exists());
            }
        }
    }
}

#[tokio::test]
async fn unreplaceable_targets_fail_before_restore() {
    let Some(fixture) = RestoreTest::new(true).await else {
        return;
    };
    let filesystem_root = fixture.root.path().ancestors().last().unwrap();
    for target in [Path::new("."), Path::new(".."), filesystem_root] {
        for dry_run in [false, true] {
            let mut command = fixture.command(&["--yes", "--force"]);
            command
                .arg("--target-data-dir")
                .arg(target)
                .env("UTOPIA_RESTORE_TEST_FAIL", "1");
            if dry_run {
                command.arg("--dry-run");
            }
            assert_failure(&command.output().unwrap(), "replaceable directory");
            assert_eq!(
                fixture.calls(),
                "",
                "invalid target reached pg_restore: {target:?}"
            );
            assert!(!fixture.target().exists());
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn dump_component_with_trailing_dot_or_slash_uses_its_normalized_file_path() {
    for dump in ["pg_dump.custom/", "pg_dump.custom/."] {
        let Some(fixture) = RestoreTest::new(false).await else {
            return;
        };
        fixture.edit_manifest(|manifest| manifest["components"]["pg_dump"]["path"] = dump.into());
        fixture.pack();
        assert_success(&fixture.restore(&["--yes", "--dry-run"]));
        assert_eq!(fixture.calls(), "");
        assert_success(&fixture.restore(&["--yes"]));
        assert_eq!(fixture.calls().lines().count(), 1);
        assert!(!fixture.target().exists());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn missing_parent_followed_by_traversal_is_rejected_before_restore() {
    let Some(fixture) = RestoreTest::new(true).await else {
        return;
    };
    fixture.old_data();
    let missing = fixture.root.path().join("missing");
    for flags in [vec!["--yes"], vec!["--yes", "--dry-run"]] {
        let output = fixture
            .command(&flags)
            .arg("--target-data-dir")
            .arg(missing.join("../target"))
            .output()
            .unwrap();
        assert_failure(&output, "target data dir");
        assert_eq!(
            fixture.calls(),
            "",
            "unresolved traversal reached pg_restore"
        );
        assert!(
            !missing.exists(),
            "preflight must not create missing parents"
        );
        fixture.assert_old_data();
    }
}

#[cfg(unix)]
#[tokio::test]
async fn force_restores_through_parent_traversal_in_an_existing_target() {
    let Some(fixture) = RestoreTest::new(true).await else {
        return;
    };
    fixture.old_data();
    let output = fixture
        .command(&["--yes", "--force"])
        .arg("--target-data-dir")
        .arg(fixture.target().join("../target"))
        .env(
            "UTOPIA_RESTORE_TEST_OLD_FILE",
            fixture.target().join("files/old.bin"),
        )
        .output()
        .unwrap();
    assert_success(&output);
    assert_eq!(fixture.calls().lines().count(), 1);
    assert!(!fixture.target().join("files/old.bin").exists());
    assert_eq!(
        std::fs::read(fixture.target().join("files/new.bin")).unwrap(),
        b"new data"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_targets_with_trailing_dot_or_slash_fail_before_restore() {
    let Some(fixture) = RestoreTest::new(true).await else {
        return;
    };
    fixture.old_data();
    std::os::unix::fs::symlink(fixture.target(), fixture.root.path().join("link")).unwrap();
    for suffix in ["link", "link/.", "link/"] {
        for dry_run in [false, true] {
            let mut command = fixture.command(&["--yes", "--force"]);
            command
                .arg("--target-data-dir")
                .arg(fixture.root.path().join(suffix))
                .env("UTOPIA_RESTORE_TEST_FAIL", "1");
            if dry_run {
                command.arg("--dry-run");
            }
            assert_failure(&command.output().unwrap(), "target data dir");
            assert_eq!(
                fixture.calls(),
                "",
                "symlink target reached pg_restore: {suffix}"
            );
            fixture.assert_old_data();
        }
    }
}

#[tokio::test]
async fn archive_component_paths_must_stay_inside_the_archive() {
    for component in ["pg_dump", "data_dir"] {
        for absolute in [false, true] {
            let Some(fixture) = RestoreTest::new(true).await else {
                return;
            };
            let outside = if absolute {
                fixture.root.path().join("outside")
            } else {
                PathBuf::from("../outside")
            };
            fixture.edit_manifest(|manifest| {
                manifest["components"][component]["path"] =
                    outside.to_string_lossy().as_ref().into();
                manifest["checksums"] = serde_json::json!({});
            });
            fixture.pack();
            for flags in [
                vec!["--yes", "--force"],
                vec!["--yes", "--force", "--dry-run"],
            ] {
                assert_failure(&fixture.restore(&flags), "invalid archive component path");
                assert_eq!(fixture.calls(), "");
                assert!(!fixture.target().exists());
            }
        }
    }
}

#[tokio::test]
async fn pg_restore_failure_preserves_the_entire_old_directory() {
    let Some(fixture) = RestoreTest::new(true).await else {
        return;
    };
    fixture.old_data();
    let output = fixture
        .command(&["--yes", "--force"])
        .env("UTOPIA_RESTORE_TEST_FAIL", "1")
        .env(
            "UTOPIA_RESTORE_TEST_OLD_FILE",
            fixture.target().join("files/old.bin"),
        )
        .output()
        .unwrap();
    assert_failure(&output, "pg_restore exited");
    assert!(String::from_utf8_lossy(&output.stderr).contains("rolled back"));
    assert_eq!(fixture.calls().lines().count(), 1);
    fixture.assert_old_data();
}

#[tokio::test]
async fn failed_data_installation_rolls_back_the_old_directory() {
    let Some(fixture) = RestoreTest::new(true).await else {
        return;
    };
    fixture.old_data();
    let output = fixture
        .command(&["--yes", "--force"])
        .env("UTOPIA_RESTORE_TEST_REMOVE_PREPARED", fixture.root.path())
        .env(
            "UTOPIA_RESTORE_TEST_OLD_FILE",
            fixture.target().join("files/old.bin"),
        )
        .output()
        .unwrap();
    assert_failure(&output, "database restored");
    assert!(String::from_utf8_lossy(&output.stderr).contains("installation failed"));
    assert_eq!(fixture.calls().lines().count(), 1);
    fixture.assert_old_data();
    for entry in std::fs::read_dir(fixture.root.path()).unwrap() {
        assert!(
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".utopia-"),
            "staging directory was not cleaned up"
        );
    }
}

#[tokio::test]
async fn empty_target_restores_normally() {
    let Some(fixture) = RestoreTest::new(true).await else {
        return;
    };
    std::fs::create_dir(fixture.target()).unwrap();
    assert_success(&fixture.restore(&["--yes"]));
    let calls = fixture.calls();
    assert_eq!(calls.lines().count(), 1);
    assert!(!calls.contains("--clean"), "--yes must not grant overwrite");
    assert!(!calls.contains("--if-exists"));
    assert!(calls.contains("--single-transaction"));
    assert_eq!(
        std::fs::read(fixture.target().join("files/new.bin")).unwrap(),
        b"new data"
    );
}

#[tokio::test]
async fn database_only_backup_leaves_existing_data_untouched() {
    let Some(fixture) = RestoreTest::new(false).await else {
        return;
    };
    fixture.old_data();
    assert_success(&fixture.restore(&["--yes", "--dry-run"]));
    assert_eq!(fixture.calls(), "");
    fixture.assert_old_data();
    assert_success(&fixture.restore(&["--yes"]));
    assert_eq!(fixture.calls().lines().count(), 1);
    fixture.assert_old_data();

    let Some(fixture) = RestoreTest::new(false).await else {
        return;
    };
    std::fs::write(
        fixture.target(),
        b"a database-only restore ignores this file",
    )
    .unwrap();
    for flags in [vec!["--yes", "--dry-run"], vec!["--yes"]] {
        assert_success(&fixture.restore(&flags));
        assert_eq!(
            std::fs::read(fixture.target()).unwrap(),
            b"a database-only restore ignores this file"
        );
    }
}

#[tokio::test]
async fn real_postgres_backup_and_restore_require_force_and_preserve_files_on_sql_failure() {
    let Some(fixture) = RestoreTest::new(true).await else {
        return;
    };
    for binary in ["pg_dump", "pg_restore"] {
        assert!(
            Command::new(binary)
                .arg("--version")
                .output()
                .is_ok_and(|output| output.status.success()),
            "{binary} is required for database-backed CLI tests; install PostgreSQL client tools"
        );
    }
    let mut connection = fixture.connection().await;
    connection
        .execute("CREATE TABLE restored (value integer); INSERT INTO restored VALUES (7)")
        .await
        .unwrap();
    // 使用环境密钥的部署即使要求备份密钥，也可能没有磁盘 secret.key。
    std::fs::remove_file(fixture.source().join("data/secret.key")).unwrap();
    let backup = |archive: &Path| {
        fixture
            .cli()
            .args([
                "backup",
                "--include-data-dir",
                "--include-secret-key",
                "--output",
            ])
            .arg(archive)
            .env("UTOPIA_SECRET_KEY", "11".repeat(32))
            .output()
            .unwrap()
    };
    let restore = |archive: &Path, target: &Path, flags: &[&str]| {
        fixture
            .cli()
            .args(["restore", "--from"])
            .arg(archive)
            .arg("--target-data-dir")
            .arg(target)
            .args(flags)
            .output()
            .unwrap()
    };
    let archive = fixture.root.path().join("real-backup.tar.gz");
    assert_success(&backup(&archive));
    connection.execute("DROP TABLE restored").await.unwrap();
    assert_success(&restore(
        &archive,
        &fixture.target(),
        &["--yes", "--dry-run"],
    ));
    let tables: i64 =
        sqlx::query_scalar("SELECT count(*) FROM pg_catalog.pg_tables WHERE schemaname = 'public'")
            .fetch_one(&mut connection)
            .await
            .unwrap();
    assert_eq!(tables, 0);
    assert!(!fixture.target().exists());
    assert_success(&restore(&archive, &fixture.target(), &["--yes"]));
    let restored: i32 = sqlx::query_scalar("SELECT value FROM restored")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(restored, 7);
    assert_eq!(
        std::fs::read(fixture.target().join("files/new.bin")).unwrap(),
        b"new data"
    );
    assert!(!fixture.target().join("secret.key").exists());

    connection
        .execute("UPDATE restored SET value = 99")
        .await
        .unwrap();
    let unused_target = fixture.root.path().join("database-guard-target");
    for flags in [vec![], vec!["--yes"], vec!["--yes", "--dry-run"]] {
        assert_failure(
            &restore(&archive, &unused_target, &flags),
            "target database is non-empty",
        );
        let unchanged: i32 = sqlx::query_scalar("SELECT value FROM restored")
            .fetch_one(&mut connection)
            .await
            .unwrap();
        assert_eq!(unchanged, 99);
        assert!(!unused_target.exists());
    }
    fixture.old_data();
    assert_success(&restore(&archive, &fixture.target(), &["--yes", "--force"]));
    let restored: i32 = sqlx::query_scalar("SELECT value FROM restored")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(restored, 7);
    assert!(!fixture.target().join("files/old.bin").exists());
    assert!(!fixture.target().join("secret.key").exists());

    // 准备连接的会话设置允许建索引；pg_restore 的新连接没有该设置，索引重建会失败。
    connection
        .execute(
            "CREATE FUNCTION fail_restore(integer) RETURNS integer
             LANGUAGE plpgsql IMMUTABLE AS $$
             BEGIN
                 IF current_setting('restore.test_source', true) IS DISTINCT FROM 'yes' THEN
                     RAISE EXCEPTION 'simulated index restore failure';
                 END IF;
                 RETURN $1;
             END $$;
             SET restore.test_source='yes';
             CREATE INDEX restore_failure ON restored (fail_restore(value))",
        )
        .await
        .unwrap();
    let failing_archive = fixture.root.path().join("failing-backup.tar.gz");
    assert_success(&backup(&failing_archive));
    connection
        .execute("UPDATE restored SET value = 99")
        .await
        .unwrap();
    std::fs::remove_file(fixture.target().join("files/new.bin")).unwrap();
    fixture.old_data();
    let failed = restore(&failing_archive, &fixture.target(), &["--yes", "--force"]);
    assert_failure(&failed, "pg_restore exited");
    assert!(String::from_utf8_lossy(&failed.stderr).contains("simulated index restore failure"));
    fixture.assert_old_data();
    // 索引重建错误应回滚整次数据库恢复，恢复前的值和原数据目录都必须保留。
    let unchanged: i32 = sqlx::query_scalar("SELECT value FROM restored")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(unchanged, 99);
    assert!(String::from_utf8_lossy(&failed.stderr).contains("rolled back"));
}
