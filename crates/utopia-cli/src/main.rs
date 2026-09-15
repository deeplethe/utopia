//! `utopia` — operator-facing command-line entry point.
//!
//! See `.roadmap-proposals/backup-restore.md` for the design rationale.
//! This is the draft PR (branch `feat/backup-restore`): `backup` is
//! implemented, `restore` is stubbed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use chrono::Utc;
use serde::Serialize;
use tracing_subscriber::EnvFilter;
use utopia_core::config::AppConfig;

#[derive(Debug)]
enum Command2 {
    Backup(BackupArgs),
    Restore(RestoreArgs),
}

#[derive(Debug, Default)]
struct BackupArgs {
    output: Option<PathBuf>,
    include_data_dir: bool,
    dry_run: bool,
    pg_dump: Option<PathBuf>,
    tar: Option<PathBuf>,
    migration_url: Option<String>,
}

#[derive(Debug, Default)]
struct RestoreArgs {
    from: Option<PathBuf>,
    target_data_dir: Option<PathBuf>,
    pg_restore: Option<PathBuf>,
    dry_run: bool,
    force: bool,
    yes: bool,
}

#[derive(Debug, Serialize)]
struct Manifest {
    schema_version: u32,
    utopia_version: &'static str,
    created_at: String,
    components: ManifestComponents,
    checksums: HashMap<String, String>,
}

#[derive(Debug, Serialize)]
struct ManifestComponents {
    pg_dump: ManifestComponent,
    data_dir: ManifestDataDir,
}

#[derive(Debug, Serialize)]
struct ManifestComponent {
    path: &'static str,
    format: &'static str,
    bytes: u64,
}

#[derive(Debug, Serialize)]
struct ManifestDataDir {
    path: String,
    present: bool,
}

fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,utopia=debug".into()),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = parse(&args)?;
    match cmd {
        Command2::Backup(a) => run_backup(a),
        Command2::Restore(a) => run_restore(a),
    }
}

fn parse(args: &[String]) -> anyhow::Result<Command2> {
    let mut iter = args.iter();
    let sub = iter
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: utopia <backup|restore> [flags]\nRun `utopia <subcommand> --help` for details."))?;
    match sub.as_str() {
        "backup" => Ok(Command2::Backup(parse_backup(&mut iter)?)),
        "restore" => Ok(Command2::Restore(parse_restore(&mut iter)?)),
        "--help" | "-h" | "help" => {
            print_help();
            std::process::exit(0);
        }
        other => anyhow::bail!("unknown subcommand `{other}` (expected `backup` or `restore`)"),
    }
}

fn print_help() {
    eprintln!(
        "utopia — operator commands\n\
         \n\
         USAGE:\n  \
             utopia <backup|restore> [flags]\n\
         \n\
         SUBCOMMANDS:\n  \
             backup   Snapshot the Postgres database (and optionally the data dir) into a tarball.\n  \
             restore  Restore from a tarball produced by `utopia backup`. (TODO: stubbed in this PR.)\n"
    );
}

fn parse_backup<'a, I: Iterator<Item = &'a String>>(
    iter: &mut I,
) -> anyhow::Result<BackupArgs> {
    let mut a = BackupArgs::default();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--output" => a.output = iter.next().map(PathBuf::from),
            "--include-data-dir" => a.include_data_dir = true,
            "--dry-run" => a.dry_run = true,
            "--pg-dump" => a.pg_dump = iter.next().map(PathBuf::from),
            "--tar" => a.tar = iter.next().map(PathBuf::from),
            "--migration-url" => a.migration_url = iter.next().cloned(),
            "--help" | "-h" => {
                eprintln!(
                    "utopia backup — snapshot the database (and optionally the data dir)\n\
                     \n\
                     FLAGS:\n      \
                         --output <path>            Final tarball path (default: ./utopia-backup-<utc>.tar.gz).\n      \
                         --include-data-dir         Add UTOPIA_DATA_DIR (files/ + index/) to the tarball.\n      \
                         --dry-run                  Plan only; print every step, write nothing.\n      \
                         --pg-dump <path>           Override the pg_dump binary (default: PATH lookup).\n      \
                         --tar <path>               Override the tar binary (default: PATH lookup).\n      \
                         --migration-url <url>      Connect as the migration role for the dump.\n"
                );
                std::process::exit(0);
            }
            other => anyhow::bail!("unknown flag `{other}` for `utopia backup`"),
        }
    }
    Ok(a)
}

fn parse_restore<'a, I: Iterator<Item = &'a String>>(
    iter: &mut I,
) -> anyhow::Result<RestoreArgs> {
    let mut a = RestoreArgs::default();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--from" => a.from = iter.next().map(PathBuf::from),
            "--target-data-dir" => a.target_data_dir = iter.next().map(PathBuf::from),
            "--pg-restore" => a.pg_restore = iter.next().map(PathBuf::from),
            "--dry-run" => a.dry_run = true,
            "--force" => a.force = true,
            "--yes" => a.yes = true,
            "--help" | "-h" => {
                eprintln!(
                    "utopia restore — restore from a backup tarball\n\
                     \n\
                     FLAGS:\n      \
                         --from <path>              Path to a tarball produced by `utopia backup`.\n      \
                         --target-data-dir <path>   Where to place the restored data dir.\n      \
                         --pg-restore <path>        Override the pg_restore binary.\n      \
                         --dry-run                  Plan only.\n      \
                         --force                    Required if target database is non-empty.\n      \
                         --yes                      Skip the confirmation prompt.\n"
                );
                std::process::exit(0);
            }
            other => anyhow::bail!("unknown flag `{other}` for `utopia restore`"),
        }
    }
    if a.from.is_none() {
        anyhow::bail!("`utopia restore` requires --from <path>");
    }
    Ok(a)
}

// ---------------------------------------------------------------------------
// backup
// ---------------------------------------------------------------------------

fn run_backup(args: BackupArgs) -> anyhow::Result<()> {
    let cfg = AppConfig::load()?;
    let pg_dump_bin = args.pg_dump.clone().unwrap_or_else(|| PathBuf::from("pg_dump"));
    let tar_bin = args.tar.clone().unwrap_or_else(|| PathBuf::from("tar"));

    let conn = args
        .migration_url
        .clone()
        .unwrap_or_else(|| cfg.migration_url().to_string());

    let data_dir = PathBuf::from(&cfg.data_dir);
    let stamp = Utc::now().format("%Y-%m-%dT%H-%M-%SZ").to_string();
    let output = args
        .output
        .clone()
        .unwrap_or_else(|| PathBuf::from(format!("utopia-backup-{stamp}.tar.gz")));

    // Plan: what we'd do, in order, with the actual resolved values.
    let plan = format!(
        "[dry-run] pg_dump binary:       {}\n[dry-run] tar binary:           {}\n[dry-run] database host:        {}\n[dry-run] data dir:             {}\n[dry-run] output tarball:       {}\n[dry-run] include data dir:     {}\n",
        pg_dump_bin.display(),
        tar_bin.display(),
        redact_url_host(&conn),
        data_dir.display(),
        output.display(),
        args.include_data_dir,
    );

    if args.dry_run {
        print!("{plan}");
        // Resolve the preconditions anyway so a bad plan returns non-zero.
        preflight_backup(&pg_dump_bin, &tar_bin, &output, &data_dir, args.include_data_dir)?;
        eprintln!("[dry-run] plan resolved cleanly; no files written.");
        return Ok(());
    }

    preflight_backup(&pg_dump_bin, &tar_bin, &output, &data_dir, args.include_data_dir)?;

    // Stage: dump Postgres to a temp file we control.
    let stage = tempdir_in(std::env::current_dir()?.as_path())?;
    let pg_dump_path = stage.join("pg_dump.custom");
    tracing::info!(path = %pg_dump_path.display(), "running pg_dump -Fc");
    run_pg_dump(&pg_dump_bin, &conn, &pg_dump_path)?;

    // Build tarball.
    tracing::info!(path = %output.display(), "writing tarball");
    build_tarball(
        &tar_bin,
        &output,
        &pg_dump_path,
        &data_dir,
        args.include_data_dir,
        &conn,
    )?;

    let _ = std::fs::remove_dir_all(&stage);
    tracing::info!(path = %output.display(), "backup complete");
    Ok(())
}

fn preflight_backup(
    pg_dump_bin: &Path,
    tar_bin: &Path,
    output: &Path,
    data_dir: &Path,
    include_data_dir: bool,
) -> anyhow::Result<()> {
    if !binary_works(pg_dump_bin) {
        anyhow::bail!(
            "pg_dump not found or not executable: {}. Install postgresql-client or pass --pg-dump <path>.",
            pg_dump_bin.display()
        );
    }
    if !binary_works(tar_bin) {
        anyhow::bail!(
            "tar not found or not executable: {}. Pass --tar <path>.",
            tar_bin.display()
        );
    }
    if output.exists() {
        anyhow::bail!(
            "refusing to overwrite existing output: {}. Move it aside or pick --output <new path>.",
            output.display()
        );
    }
    if include_data_dir && !data_dir.exists() {
        anyhow::bail!(
            "--include-data-dir was set but UTOPIA_DATA_DIR does not exist: {}",
            data_dir.display()
        );
    }
    Ok(())
}

fn binary_works(bin: &Path) -> bool {
    Command::new(bin)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn run_pg_dump(bin: &Path, conn: &str, out: &Path) -> anyhow::Result<()> {
    let status = Command::new(bin)
        .arg("-Fc")
        .arg("--dbname")
        .arg(conn)
        .arg("--file")
        .arg(out)
        .stdin(Stdio::null())
        .status()?;
    if !status.success() {
        anyhow::bail!(
            "pg_dump exited with status {}; check credentials and that the database is reachable",
            status
        );
    }
    Ok(())
}

fn build_tarball(
    tar_bin: &Path,
    output: &Path,
    pg_dump_path: &Path,
    data_dir: &Path,
    include_data_dir: bool,
    conn: &str,
) -> anyhow::Result<()> {
    // Write manifest.json next to the dump inside a staging dir, then tar
    // the staging dir into the final tarball.
    let stage = output
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(format!(
            ".utopia-backup-stage-{}",
            Utc::now().timestamp_millis()
        ));
    std::fs::create_dir_all(&stage)?;
    let manifest_path = stage.join("manifest.json");
    let manifest = Manifest {
        schema_version: 1,
        utopia_version: env!("CARGO_PKG_VERSION"),
        created_at: Utc::now().to_rfc3339(),
        components: ManifestComponents {
            pg_dump: ManifestComponent {
                path: "pg_dump.custom",
                format: "pg_dump -Fc",
                bytes: std::fs::metadata(pg_dump_path).map(|m| m.len()).unwrap_or(0),
            },
            data_dir: ManifestDataDir {
                path: "data".to_string(),
                present: include_data_dir,
            },
        },
        checksums: HashMap::from([(
            "pg_dump.custom".to_string(),
            format!("sha256:{}", sha256_file(pg_dump_path)?),
        )]),
    };
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    // Move the dump into the stage.
    std::fs::copy(pg_dump_path, stage.join("pg_dump.custom"))?;
    if include_data_dir {
        copy_dir_recursive(data_dir, &stage.join("data"))?;
    }
    // tar -C <stage> -czf <output> .
    let status = Command::new(tar_bin)
        .arg("-C")
        .arg(&stage)
        .arg("-czf")
        .arg(output)
        .arg(".")
        .stdin(Stdio::null())
        .status()?;
    let _ = std::fs::remove_dir_all(&stage);
    if !status.success() {
        anyhow::bail!("tar exited with status {}", status);
    }
    // Touch the connection-string metadata via tracing; not embedded in the
    // archive on purpose (the dump file may be enough).
    tracing::debug!(conn_host = %redact_url_host(conn), "tarball built");
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if ty.is_symlink() {
            // Skip symlinks — they don't survive backup/restore in a portable way.
            tracing::warn!(path = %from.display(), "skipping symlink in data dir");
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn tempdir_in(parent: &Path) -> anyhow::Result<PathBuf> {
    let name = format!(".utopia-backup-{}", Utc::now().timestamp_millis());
    let p = parent.join(name);
    std::fs::create_dir_all(&p)?;
    Ok(p)
}

/// Replace everything after the `://` up to the next `@` with `***`.
fn redact_url_host(conn: &str) -> String {
    match (conn.find("://"), conn.find('@')) {
        (Some(scheme), Some(at)) if at > scheme => {
            let mut out = String::with_capacity(conn.len());
            out.push_str(&conn[..scheme + 3]);
            out.push_str("***");
            out.push_str(&conn[at..]);
            out
        }
        _ => "<no host>".to_string(),
    }
}

fn sha256_file(path: &Path) -> anyhow::Result<String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let digest = hasher.finalize();
    Ok(hex_encode(&digest))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

// ---------------------------------------------------------------------------
// restore (stubbed)
// ---------------------------------------------------------------------------

fn run_restore(args: RestoreArgs) -> anyhow::Result<()> {
    let cfg = AppConfig::load()?;
    let from = args
        .from
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("--from <path> is required"))?;
    if !from.exists() {
        anyhow::bail!("backup archive does not exist: {}", from.display());
    }

    let target_db = cfg.migration_url().to_string();
    let target_data_dir = args
        .target_data_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from(&cfg.data_dir));

    eprintln!(
        "Restore plan (NOT YET IMPLEMENTED):\n  \
         source archive:       {}\n  \
         target database:      {}\n  \
         target data dir:      {}\n  \
         force:                {}\n  \
         dry-run:              {}\n",
        from.display(),
        redact_url_host(&target_db),
        target_data_dir.display(),
        args.force,
        args.dry_run,
    );

    // TODO(backup-restore): implement the real restore path. See
    // `.roadmap-proposals/backup-restore.md` §4.5 for the staging/swap design
    // and §6 Q1/Q2 for the open design questions that gate this.
    anyhow::bail!(
        "utopia restore is not implemented yet on this branch (feat/backup-restore). \
         See `.roadmap-proposals/backup-restore.md` for the design."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_backup_minimal() {
        let args = vec!["backup".to_string()];
        let cmd = parse(&args).expect("parses");
        match cmd {
            Command2::Backup(b) => {
                assert!(b.output.is_none());
                assert!(!b.include_data_dir);
                assert!(!b.dry_run);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parses_backup_full() {
        let args = vec![
            "backup".to_string(),
            "--output".to_string(),
            "/tmp/b.tar.gz".to_string(),
            "--include-data-dir".to_string(),
            "--dry-run".to_string(),
            "--pg-dump".to_string(),
            "/opt/pg/bin/pg_dump".to_string(),
            "--tar".to_string(),
            "/bin/tar".to_string(),
            "--migration-url".to_string(),
            "postgres://u:***@h/db".to_string(),
        ];
        let cmd = parse(&args).expect("parses");
        match cmd {
            Command2::Backup(b) => {
                assert_eq!(b.output, Some(PathBuf::from("/tmp/b.tar.gz")));
                assert!(b.include_data_dir);
                assert!(b.dry_run);
                assert_eq!(b.pg_dump, Some(PathBuf::from("/opt/pg/bin/pg_dump")));
                assert_eq!(b.tar, Some(PathBuf::from("/bin/tar")));
                assert_eq!(
                    b.migration_url.as_deref(),
                    Some("postgres://u:***@h/db")
                );
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parses_restore_requires_from() {
        let args = vec!["restore".to_string()];
        assert!(parse(&args).is_err());
    }

    #[test]
    fn parses_restore_full() {
        let args = vec![
            "restore".to_string(),
            "--from".to_string(),
            "/tmp/b.tar.gz".to_string(),
            "--target-data-dir".to_string(),
            "/var/lib/utopia/data".to_string(),
            "--force".to_string(),
            "--yes".to_string(),
            "--dry-run".to_string(),
        ];
        let cmd = parse(&args).expect("parses");
        match cmd {
            Command2::Restore(r) => {
                assert_eq!(r.from, Some(PathBuf::from("/tmp/b.tar.gz")));
                assert_eq!(r.target_data_dir, Some(PathBuf::from("/var/lib/utopia/data")));
                assert!(r.force);
                assert!(r.yes);
                assert!(r.dry_run);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn rejects_unknown_subcommand() {
        let args = vec!["frobnicate".to_string()];
        assert!(parse(&args).is_err());
    }

    #[test]
    fn redact_url_host_keeps_userinfo_at_host() {
        let r = redact_url_host("postgres://utopia:secret@db:5432/utopia");
        assert_eq!(r, "postgres://***@db:5432/utopia");
    }

    #[test]
    fn redact_url_host_handles_no_at() {
        let r = redact_url_host("not a url");
        assert_eq!(r, "<no host>");
    }

    #[test]
    fn hex_encode_known_value() {
        // SHA256 of "abc"
        assert_eq!(
            hex_encode(&[
                0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
                0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
                0xf2, 0x00, 0x15, 0xad,
            ]),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
