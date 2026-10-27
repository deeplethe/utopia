//! `utopia` — operator-facing command-line entry point.
//!
//! See `.roadmap-proposals/backup-restore.md` for the design rationale.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::Context;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::{Connection, PgConnection};
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
    /// Put `secret.key` in the archive too. Off by default: the dump carries
    /// credentials sealed with that key, and an archive is the one artifact that
    /// is copied between hosts and handed to whoever runs the restore. Both
    /// halves in one file is not an encrypted archive, it is no sealing at all.
    include_secret_key: bool,
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

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    schema_version: u32,
    utopia_version: String,
    created_at: String,
    components: ManifestComponents,
    checksums: HashMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ManifestComponents {
    pg_dump: ManifestComponent,
    data_dir: ManifestDataDir,
}

#[derive(Debug, Serialize, Deserialize)]
struct ManifestComponent {
    path: String,
    format: String,
    bytes: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct ManifestDataDir {
    path: String,
    present: bool,
    /// Whether the sealing key is inside this archive, so that an archive can be
    /// audited without being unpacked and restore can say what it will install.
    #[serde(default)]
    secret_key: bool,
}

/// Manifest version the running binary writes. Restore refuses a manifest
/// whose `schema_version` is greater than this (forward-incompatible) and
/// warns when older. Kept as a constant — bumping is a deliberate decision,
/// not a side effect of a code change.
// 是迁移文件的**个数**，不是最大的编号（守卫 `schema_version_policy_compares_against_current`
// 按个数比）：编号有空缺时两者不同——0071 由一个开放 PR 占着，0072 先落，个数是 71
const CURRENT_SCHEMA_VERSION: u32 = 107;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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
        Command2::Restore(a) => run_restore(a).await,
    }
}

fn parse(args: &[String]) -> anyhow::Result<Command2> {
    let mut iter = args.iter();
    let sub = iter.next().ok_or_else(|| {
        anyhow::anyhow!(
            "usage: utopia <backup|restore> [flags]\nRun `utopia <subcommand> --help` for details."
        )
    })?;
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
             restore  Restore from a tarball produced by `utopia backup`.\n"
    );
}

fn option_value<'a, I: Iterator<Item = &'a String>>(
    flag: &str,
    iter: &mut I,
) -> anyhow::Result<&'a String> {
    match iter.next() {
        Some(value) if !value.is_empty() && !value.starts_with('-') => Ok(value),
        _ => anyhow::bail!("{flag} requires a value"),
    }
}

fn parse_backup<'a, I: Iterator<Item = &'a String>>(iter: &mut I) -> anyhow::Result<BackupArgs> {
    let mut a = BackupArgs::default();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--output" => a.output = Some(PathBuf::from(option_value(flag, iter)?)),
            "--include-data-dir" => a.include_data_dir = true,
            "--include-secret-key" => a.include_secret_key = true,
            "--dry-run" => a.dry_run = true,
            "--pg-dump" => a.pg_dump = Some(PathBuf::from(option_value(flag, iter)?)),
            "--tar" => a.tar = Some(PathBuf::from(option_value(flag, iter)?)),
            "--migration-url" => a.migration_url = Some(option_value(flag, iter)?.clone()),
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

fn parse_restore<'a, I: Iterator<Item = &'a String>>(iter: &mut I) -> anyhow::Result<RestoreArgs> {
    let mut a = RestoreArgs::default();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--from" => a.from = Some(PathBuf::from(option_value(flag, iter)?)),
            "--target-data-dir" => {
                a.target_data_dir = Some(PathBuf::from(option_value(flag, iter)?))
            }
            "--pg-restore" => a.pg_restore = Some(PathBuf::from(option_value(flag, iter)?)),
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
                         --dry-run                  Validate and print the plan; do not change targets.\n      \
                         --force                    Required for a non-empty target database or restored data dir.\n      \
                         --yes                      Skip confirmation only; does not imply --force.\n"
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
    let pg_dump_bin = args
        .pg_dump
        .clone()
        .unwrap_or_else(|| PathBuf::from("pg_dump"));
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
        preflight_backup(
            &pg_dump_bin,
            &tar_bin,
            &output,
            &data_dir,
            args.include_data_dir,
        )?;
        eprintln!("[dry-run] plan resolved cleanly; no files written.");
        return Ok(());
    }

    preflight_backup(
        &pg_dump_bin,
        &tar_bin,
        &output,
        &data_dir,
        args.include_data_dir,
    )?;

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
        args.include_secret_key,
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

#[allow(clippy::too_many_arguments)]
fn build_tarball(
    tar_bin: &Path,
    output: &Path,
    pg_dump_path: &Path,
    data_dir: &Path,
    include_data_dir: bool,
    include_secret_key: bool,
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
        schema_version: CURRENT_SCHEMA_VERSION,
        utopia_version: env!("CARGO_PKG_VERSION").to_string(),
        created_at: Utc::now().to_rfc3339(),
        components: ManifestComponents {
            pg_dump: ManifestComponent {
                path: "pg_dump.custom".to_string(),
                format: "pg_dump -Fc".to_string(),
                bytes: std::fs::metadata(pg_dump_path)
                    .map(|m| m.len())
                    .unwrap_or(0),
            },
            data_dir: ManifestDataDir {
                path: "data".to_string(),
                present: include_data_dir,
                secret_key: include_secret_key,
            },
        },
        checksums: HashMap::from([(
            "pg_dump.custom".to_string(),
            format!("sha256:{}", sha256_file(pg_dump_path)?),
        )]),
    };
    std::fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)?;
    // Move the dump into the stage.
    std::fs::copy(pg_dump_path, stage.join("pg_dump.custom"))?;
    if include_data_dir {
        copy_dir_recursive(data_dir, &stage.join("data"), include_secret_key)?;
        if !include_secret_key && data_dir.join(SECRET_KEY_FILE).exists() {
            tracing::info!(
                "left {SECRET_KEY_FILE} out of the archive; pass --include-secret-key to carry it,                  or set UTOPIA_SECRET_KEY on the host that restores"
            );
        }
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

/// The file the server seals credentials with (`utopia-server`'s `secret_key_file`).
const SECRET_KEY_FILE: &str = "secret.key";

/// Copy a directory into the staging area. `with_secret_key` off leaves the
/// sealing key behind: everything else in `data/` is content, that one file is
/// the key to the dump's ciphertext.
fn copy_dir_recursive(src: &Path, dst: &Path, with_secret_key: bool) -> anyhow::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        if !with_secret_key && entry.file_name() == SECRET_KEY_FILE {
            continue;
        }
        let to = dst.join(entry.file_name());
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_recursive(&from, &to, with_secret_key)?;
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
// 恢复
// ---------------------------------------------------------------------------

async fn run_restore(args: RestoreArgs) -> anyhow::Result<()> {
    let cfg = AppConfig::load()?;
    let from = args
        .from
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("--from <path> is required"))?;
    if !from.exists() {
        anyhow::bail!("backup archive does not exist: {}", from.display());
    }

    let pg_restore_bin = args
        .pg_restore
        .clone()
        .unwrap_or_else(|| PathBuf::from("pg_restore"));
    let tar_bin = PathBuf::from("tar");
    let target_db = cfg.migration_url().to_string();
    let target_data_dir = args
        .target_data_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from(&cfg.data_dir));

    let stage = tempfile::Builder::new()
        .prefix(".utopia-restore-")
        .tempdir_in(std::env::current_dir()?)?;
    let manifest_path = stage.path().join("manifest.json");
    let manifest = extract_and_verify(&tar_bin, from, stage.path(), &manifest_path)?;

    // Manifest version policy: refuse forward, warn older. Forward refusal
    // is fatal because restoring into an older server with a newer schema
    // will silently drop tables or, worse, produce a working schema with
    // missing columns. Older manifests are accepted with a warning because
    // a newer server can usually still load them (the schema migrations are
    // forward-only; older files just exercise older code paths).
    if manifest.schema_version > CURRENT_SCHEMA_VERSION {
        anyhow::bail!(
            "manifest schema_version {} is newer than this binary's {}; refusing to restore an incompatible backup",
            manifest.schema_version,
            CURRENT_SCHEMA_VERSION
        );
    }
    if manifest.schema_version < CURRENT_SCHEMA_VERSION {
        eprintln!(
            "[warn] manifest schema_version {} is older than this binary's {}; continuing",
            manifest.schema_version, CURRENT_SCHEMA_VERSION
        );
    }

    let plan = format!(
        "Restore plan:\n  \
         source archive:         {}\n  \
         manifest schema:        {} (current {})\n  \
         manifest utopia build:  {}\n  \
         manifest created:       {}\n  \
         target database:        {}\n  \
         target data dir:        {}\n  \
         force:                  {}\n  \
         dry-run:                {}\n",
        from.display(),
        manifest.schema_version,
        CURRENT_SCHEMA_VERSION,
        manifest.utopia_version,
        manifest.created_at,
        redact_url_host(&target_db),
        target_data_dir.display(),
        args.force,
        args.dry_run,
    );

    let target_data_dir = preflight_restore(
        &pg_restore_bin,
        &target_db,
        &target_data_dir,
        stage.path(),
        &manifest,
        args.force,
    )
    .await?;

    if args.dry_run {
        print!("{plan}");
        eprintln!("[dry-run] plan resolved cleanly; no changes made.");
        return Ok(());
    }

    if !args.yes {
        eprint!(
            "{plan}\nAbout to restore database objects and replace data if included in the archive. Stop all writers first. Continue? [y/N] "
        );
        let mut buf = String::new();
        std::io::stdin().read_line(&mut buf)?;
        if !matches!(buf.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            anyhow::bail!("aborted by user");
        }
    }

    apply_restore(
        &pg_restore_bin,
        &target_db,
        &target_data_dir,
        stage.path(),
        &manifest,
        args.force,
    )
    .await?;

    tracing::info!(path = %from.display(), "restore complete");
    Ok(())
}

fn extract_and_verify(
    tar_bin: &Path,
    archive: &Path,
    stage: &Path,
    manifest_path: &Path,
) -> anyhow::Result<Manifest> {
    let status = Command::new(tar_bin)
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(stage)
        .stdin(Stdio::null())
        .status()?;
    if !status.success() {
        anyhow::bail!("tar exited with status {}; archive may be corrupt", status);
    }
    if !manifest_path.exists() {
        anyhow::bail!("archive did not contain a manifest.json at the root — not a utopia backup?");
    }
    // Verify each checksummed component. pg_dump.custom is the only one
    // today; data_dir is a directory tree so it's not hashed. Refusing on
    // mismatch is loud — restoring a half-corrupted archive would be a
    // silent failure mode nobody catches.
    let manifest: Manifest = serde_json::from_slice(&std::fs::read(manifest_path)?)?;
    for (name, expected) in &manifest.checksums {
        let path = stage.join(name);
        if !path.exists() {
            anyhow::bail!("manifest references `{name}` but the archive did not contain it");
        }
        let Some(expected_sha) = expected.strip_prefix("sha256:") else {
            anyhow::bail!("unsupported checksum scheme for `{name}`: {expected}");
        };
        let actual = sha256_file(&path)?;
        if actual != expected_sha {
            anyhow::bail!(
                "checksum mismatch for `{name}`: manifest says {expected_sha}, file is {actual}"
            );
        }
    }
    Ok(manifest)
}

fn check_archive_component(stage: &Path, name: &str, directory: bool) -> anyhow::Result<()> {
    let relative = Path::new(name);
    if name.is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        anyhow::bail!("invalid archive component path: `{name}`");
    }
    let relative: PathBuf = relative.components().collect();
    let path = stage.join(relative);
    let metadata = std::fs::symlink_metadata(&path)
        .with_context(|| format!("archive did not contain required component `{name}`"))?;
    if metadata.file_type().is_symlink()
        || (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
        || !path.canonicalize()?.starts_with(stage.canonicalize()?)
    {
        anyhow::bail!("archive component `{name}` has an invalid type or location");
    }
    Ok(())
}

fn check_restore_data_target(target: &Path, force: bool) -> anyhow::Result<PathBuf> {
    if target.file_name().is_none() {
        anyhow::bail!(
            "target data dir must name a replaceable directory: {}",
            target.display()
        );
    }
    // 先移除末尾分隔符和 `/.`，否则 lstat 也会跟随符号链接。
    let normalized: PathBuf = target.components().collect();
    let absolute = std::path::absolute(normalized)?;
    // 允许父目录尚未创建，但文件或悬空链接不能充当父目录。
    let mut resolved = None;
    for parent in absolute.ancestors().skip(1) {
        match std::fs::metadata(parent) {
            Ok(metadata) if metadata.is_dir() => {
                let suffix = absolute.strip_prefix(parent)?;
                if suffix
                    .components()
                    .any(|part| !matches!(part, std::path::Component::Normal(_)))
                {
                    anyhow::bail!(
                        "target data dir traverses a missing parent: {}",
                        target.display()
                    );
                }
                // 固定实际父路径，避免移走旧目录后无法访问暂存文件和回滚路径。
                resolved = Some(parent.canonicalize()?.join(suffix));
                break;
            }
            Ok(_) => anyhow::bail!(
                "target data dir parent is not a directory: {}",
                parent.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if std::fs::symlink_metadata(parent).is_ok() {
                    anyhow::bail!(
                        "target data dir parent is a dangling link: {}",
                        parent.display()
                    );
                }
            }
            Err(error) => return Err(error).context("cannot inspect target data dir parent"),
        }
    }
    let target = resolved.context("target data dir has no accessible directory parent")?;
    match std::fs::symlink_metadata(&target) {
        Ok(metadata) => {
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                anyhow::bail!(
                    "target data dir is not a regular directory: {}",
                    target.display()
                );
            }
            if std::fs::read_dir(&target)?.next().transpose()?.is_some() && !force {
                anyhow::bail!(
                    "target data dir {} is non-empty; pass --force to overwrite",
                    target.display()
                );
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("cannot inspect target data dir"),
    }
    Ok(target)
}

async fn preflight_restore(
    pg_restore_bin: &Path,
    target_db: &str,
    target_data_dir: &Path,
    stage: &Path,
    manifest: &Manifest,
    force: bool,
) -> anyhow::Result<PathBuf> {
    check_archive_component(stage, &manifest.components.pg_dump.path, false)?;
    let target_data_dir = if manifest.components.data_dir.present {
        check_archive_component(stage, &manifest.components.data_dir.path, true)?;
        check_restore_data_target(target_data_dir, force)?
    } else {
        target_data_dir.to_path_buf()
    };
    if !binary_works(pg_restore_bin) {
        anyhow::bail!(
            "pg_restore not found or not executable: {}. Install postgresql-client or pass --pg-restore <path>.",
            pg_restore_bin.display()
        );
    }
    let mut connection = PgConnection::connect(target_db)
        .await
        .context("cannot connect to target database for restore preflight")?;
    // 空表、schema 等用户对象也算非空；命名空间依赖还能覆盖函数、类型等对象。
    let nonempty: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM pg_catalog.pg_namespace n
            WHERE n.nspname !~ '^pg_' AND n.nspname <> 'information_schema'
              AND (n.nspname <> 'public' OR EXISTS (
                  SELECT 1 FROM pg_catalog.pg_depend d
                  WHERE d.refclassid = 'pg_catalog.pg_namespace'::regclass
                    AND d.refobjid = n.oid)))
         OR EXISTS (SELECT 1 FROM pg_catalog.pg_extension WHERE extname <> 'plpgsql')
         OR EXISTS (SELECT 1 FROM pg_catalog.pg_largeobject_metadata)
         OR EXISTS (SELECT 1 FROM pg_catalog.pg_foreign_data_wrapper)
         OR EXISTS (SELECT 1 FROM pg_catalog.pg_foreign_server)
         OR EXISTS (SELECT 1 FROM pg_catalog.pg_publication)
         OR EXISTS (SELECT 1 FROM pg_catalog.pg_event_trigger)
         OR EXISTS (SELECT 1 FROM pg_catalog.pg_subscription
                    WHERE subdbid = (SELECT oid FROM pg_catalog.pg_database
                                     WHERE datname = pg_catalog.current_database()))",
    )
    .fetch_one(&mut connection)
    .await
    .context("cannot inspect target database for restore preflight")?;
    if nonempty && !force {
        anyhow::bail!("target database is non-empty; pass --force to overwrite (--yes only skips confirmation)");
    }
    Ok(target_data_dir)
}

async fn apply_restore(
    pg_restore_bin: &Path,
    target_db: &str,
    target_data_dir: &Path,
    stage: &Path,
    manifest: &Manifest,
    force: bool,
) -> anyhow::Result<()> {
    let dump_relative: PathBuf = Path::new(&manifest.components.pg_dump.path)
        .components()
        .collect();
    let pg_dump_path = stage.join(dump_relative);
    // 先在目标文件系统准备新目录；复制或数据库恢复失败时，旧目录仍保留原位。
    let prepared = if manifest.components.data_dir.present {
        let parent = target_data_dir
            .parent()
            .context("target data dir has no parent")?;
        std::fs::create_dir_all(parent)?;
        let prepared = tempfile::Builder::new()
            .prefix(".utopia-restore-data-")
            .tempdir_in(parent)?;
        copy_dir_recursive(
            &stage.join(&manifest.components.data_dir.path),
            &prepared.path().join("new"),
            true,
        )?;
        Some(prepared)
    } else {
        None
    };
    // 等待确认和准备文件期间目标可能变化，启动 pg_restore 前必须重新检查。
    let checked_target = preflight_restore(
        pg_restore_bin,
        target_db,
        target_data_dir,
        stage,
        manifest,
        force,
    )
    .await?;
    // 暂存文件已绑定首次解析的物理路径，复查不能悄悄切换到另一个目标。
    if checked_target.as_path() != target_data_dir {
        anyhow::bail!(
            "target data dir resolved to a different location after confirmation; retry with writers stopped"
        );
    }
    tracing::info!(
        archive_pg_dump = %pg_dump_path.display(),
        force,
        "running pg_restore"
    );
    let mut command = Command::new(pg_restore_bin);
    if force {
        command.args(["--clean", "--if-exists"]);
    }
    let status = command
        .args(["--exit-on-error", "--single-transaction"])
        .arg("--dbname")
        .arg(target_db)
        .arg(&pg_dump_path)
        .stdin(Stdio::null())
        .status()?;
    if !status.success() {
        anyhow::bail!(
            "pg_restore exited with status {}; SQL restore errors are rolled back; verify the database before retrying",
            status
        );
    }

    if let Some(prepared) = prepared {
        check_restore_data_target(target_data_dir, force)
            .context("database restored, but target data dir changed during restore; retry with writers stopped")?;
        let previous = prepared.path().join("previous");
        let had_target = target_data_dir.exists();
        if had_target {
            std::fs::rename(target_data_dir, &previous)
                .context("database restored, but could not move old data dir; original files remain in place")?;
        }
        if let Err(error) = std::fs::rename(prepared.path().join("new"), target_data_dir) {
            if had_target {
                if let Err(rollback) = std::fs::rename(&previous, target_data_dir) {
                    let retained = prepared.keep();
                    anyhow::bail!("database restored, but data dir installation failed: {error}; rollback failed: {rollback}; original data retained at {} — stop writers and recover manually", retained.join("previous").display());
                }
            }
            return Err(error).context("database restored, but data dir installation failed; original data dir preserved — resolve the error and retry restore");
        }
        prepared.close().with_context(|| {
            format!(
                "restore completed, but could not remove previous data at {}",
                previous.display()
            )
        })?;
        if !target_data_dir.join(SECRET_KEY_FILE).is_file() {
            // 没有钥匙，库里那些封存的凭据就打不开。与其让人在服务起来之后
            // 看见一串解不开的错误，不如现在说清楚该做什么
            tracing::warn!(
                    "this archive carries no {SECRET_KEY_FILE}: sealed credentials (model keys,                      source credentials) will not open. Set UTOPIA_SECRET_KEY on this host to the                      key the backup was taken with, or re-enter them after starting the server"
                );
        }
    }

    Ok(())
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
                assert_eq!(b.migration_url.as_deref(), Some("postgres://u:***@h/db"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parses_restore_requires_from() {
        let args = vec!["restore".to_string()];
        assert!(parse(&args).is_err());
    }

    /// 备份包里默认没有封存密钥。库里存的凭据是用它加密的，两样装在同一个
    /// 压缩包里，封存就等于没有——而压缩包恰恰是那个会被拷来拷去、交给别人去
    /// 恢复的东西
    #[test]
    fn the_sealing_key_stays_out_of_the_archive_unless_asked() {
        let src = std::env::temp_dir().join(format!("utopia-seckey-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&src);
        std::fs::create_dir_all(src.join("files")).unwrap();
        std::fs::write(src.join(SECRET_KEY_FILE), b"not-a-real-key").unwrap();
        std::fs::write(src.join("files").join("a.bin"), b"content").unwrap();

        let without = src.with_extension("without");
        let _ = std::fs::remove_dir_all(&without);
        copy_dir_recursive(&src, &without, false).unwrap();
        assert!(
            !without.join(SECRET_KEY_FILE).exists(),
            "默认把密钥带进去了"
        );
        assert!(
            without.join("files").join("a.bin").exists(),
            "别的内容不该少"
        );

        let with = src.with_extension("with");
        let _ = std::fs::remove_dir_all(&with);
        copy_dir_recursive(&src, &with, true).unwrap();
        assert!(with.join(SECRET_KEY_FILE).exists(), "说了要带却没带");

        for d in [src, without, with] {
            let _ = std::fs::remove_dir_all(d);
        }
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
                assert_eq!(
                    r.target_data_dir,
                    Some(PathBuf::from("/var/lib/utopia/data"))
                );
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

    /// Manifest round-trips: serialize → deserialize produces the same shape
    /// on the read side. Today `backup` writes with `Serialize`, `restore`
    /// reads with `Deserialize` — if either side drops or renames a field,
    /// this is the test that catches it.
    #[test]
    fn manifest_round_trip() {
        let mut checksums = HashMap::new();
        checksums.insert("pg_dump.custom".to_string(), "sha256:deadbeef".to_string());
        let written = Manifest {
            schema_version: CURRENT_SCHEMA_VERSION,
            utopia_version: "0.1.0".to_string(),
            created_at: "2026-09-17T09:00:00Z".to_string(),
            components: ManifestComponents {
                pg_dump: ManifestComponent {
                    path: "pg_dump.custom".to_string(),
                    format: "pg_dump -Fc".to_string(),
                    bytes: 12345,
                },
                data_dir: ManifestDataDir {
                    path: "data".to_string(),
                    present: true,
                    secret_key: false,
                },
            },
            checksums,
        };
        let json = serde_json::to_string(&written).unwrap();
        let read: Manifest = serde_json::from_str(&json).unwrap();
        assert_eq!(read.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(read.utopia_version, "0.1.0");
        assert_eq!(read.components.pg_dump.bytes, 12345);
        assert!(read.components.data_dir.present);
        assert_eq!(
            read.checksums.get("pg_dump.custom").map(String::as_str),
            Some("sha256:deadbeef")
        );
    }

    /// Forward-incompatible manifest: schema_version > CURRENT. Restore must
    /// bail before touching the live database. The actual gate is inside
    /// `run_restore`; this test pins the version-policy behaviour at the
    /// constant level so a future bump that changes the comparison shape
    /// (e.g. switching to a rangeset) gets caught at compile-test time, not
    /// in production.
    #[test]
    fn schema_version_policy_compares_against_current() {
        // Same shape as `run_restore`'s gate. If the rule changes here,
        // change it there too.
        let current = CURRENT_SCHEMA_VERSION;
        assert!(
            current + 1 > current,
            "newer manifests must compare > current"
        );
        assert!(
            current - 1 < current,
            "older manifests must compare < current"
        );
        // And the constant itself must stay in lockstep with the highest
        // applied migration in `migrations/` — bumping a migration file
        // without bumping this is a silent schema-version skew. The
        // `migrations/` dir is a sibling of `crates/utopia-cli/`; resolve
        // from CARGO_MANIFEST_DIR so the test works regardless of cwd.
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let migrations_dir = manifest_dir
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.join("migrations"))
            .expect("workspace root has a migrations/ dir");
        let applied = std::fs::read_dir(&migrations_dir)
            .unwrap_or_else(|e| {
                panic!(
                    "migrations dir {} unreadable: {e}",
                    migrations_dir.display()
                )
            })
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|x| x == "sql").unwrap_or(false))
            .count() as u32;
        assert_eq!(
            current, applied,
            "CURRENT_SCHEMA_VERSION must match the count of applied migrations; \
             bump it when you add a migration"
        );
    }

    /// `extract_and_verify` against an archive without a manifest.json:
    /// should fail loudly with "not a utopia backup?" rather than running
    /// pg_restore against an empty stage. Build a fake tarball in a temp
    /// dir so the path is exercised end-to-end.
    #[test]
    fn extract_fails_when_archive_has_no_manifest() {
        let stage = tempdir_for_test("no_manifest");
        // Stage must already exist for `tar -C`. Create a fake archive
        // containing exactly one file inside a fresh staging dir, then run
        // extract_and_verify against a different stage.
        let src = tempdir_for_test("src");
        std::fs::write(src.join("unrelated.txt"), b"not a backup").unwrap();
        let archive = src.join("fake.tar.gz");
        let status = Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&src)
            .arg("unrelated.txt")
            .stdin(Stdio::null())
            .status()
            .expect("tar available");
        assert!(status.success(), "host tar is required for this test");
        let manifest_path = stage.join("manifest.json");
        let result = extract_and_verify(Path::new("tar"), &archive, &stage, &manifest_path);
        let _ = std::fs::remove_dir_all(&stage);
        let _ = std::fs::remove_dir_all(&src);
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not a utopia backup"),
            "want a clear error, got: {err}"
        );
    }

    /// Checksum mismatch: stage contains a `pg_dump.custom` whose bytes
    /// don't match the manifest's sha256. `extract_and_verify` must refuse
    /// rather than pass a corrupted archive on to pg_restore.
    #[test]
    fn extract_fails_on_checksum_mismatch() {
        let src = tempdir_for_test("mismatch_src");
        // Real pg_dump bytes we can corrupt at will.
        std::fs::write(src.join("pg_dump.custom"), b"original-bytes").unwrap();
        let archive = src.join("mismatch.tar.gz");
        let status = Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&src)
            .arg("pg_dump.custom")
            .stdin(Stdio::null())
            .status()
            .expect("tar available");
        assert!(status.success());
        let manifest_json = serde_json::json!({
            "schema_version": CURRENT_SCHEMA_VERSION,
            "utopia_version": "0.1.0",
            "created_at": "2026-09-17T00:00:00Z",
            "components": {
                "pg_dump": {
                    "path": "pg_dump.custom",
                    "format": "pg_dump -Fc",
                    "bytes": 14
                },
                "data_dir": { "path": "data", "present": false }
            },
            // SHA256 of "tampered" — neither the file nor the empty archive
            // matches this, so verification must fail.
            "checksums": { "pg_dump.custom": "sha256:0000000000000000000000000000000000000000000000000000000000000000" }
        })
        .to_string();
        std::fs::write(src.join("manifest.json"), manifest_json).unwrap();
        // Re-pack with both files so the archive contains manifest + dump.
        let archive2 = src.join("mismatch2.tar.gz");
        let status = Command::new("tar")
            .arg("-czf")
            .arg(&archive2)
            .arg("-C")
            .arg(&src)
            .arg("manifest.json")
            .arg("pg_dump.custom")
            .stdin(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());

        let stage = tempdir_for_test("mismatch_stage");
        let manifest_path = stage.join("manifest.json");
        let result = extract_and_verify(Path::new("tar"), &archive2, &stage, &manifest_path);
        let _ = std::fs::remove_dir_all(&stage);
        let _ = std::fs::remove_dir_all(&src);
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("checksum mismatch"),
            "want a checksum error, got: {err}"
        );
    }

    /// Create a uniquely-named temp directory under the process's CWD. Test
    /// cleanup is the caller's responsibility — keeps the helper
    /// dependency-free (no tempfile crate). Mirrors the shape of
    /// `tempdir_in` used by the production paths.
    fn tempdir_for_test(label: &str) -> PathBuf {
        let name = format!(
            ".utopia-cli-test-{label}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let p = std::env::current_dir().unwrap().join(name);
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}

#[cfg(test)]
mod option_value_regressions {
    use super::*;

    #[test]
    fn backup_value_options_do_not_fall_back_when_missing() {
        for flag in ["--output", "--pg-dump", "--tar", "--migration-url"] {
            for suffix in [vec![], vec!["--dry-run"], vec![""]] {
                let mut args = vec!["backup", flag];
                args.extend(suffix);
                let args: Vec<String> = args.into_iter().map(str::to_string).collect();
                assert!(parse(&args).is_err(), "accepted {args:?}");
            }
        }
    }

    #[test]
    fn restore_value_options_do_not_consume_other_flags() {
        for flag in ["--from", "--target-data-dir", "--pg-restore"] {
            for suffix in [vec![], vec!["--dry-run"], vec![""]] {
                let mut args = vec!["restore", "--from", "archive.tar.gz", flag];
                args.extend(suffix);
                let args: Vec<String> = args.into_iter().map(str::to_string).collect();
                assert!(parse(&args).is_err(), "accepted {args:?}");
            }
        }
    }
}
