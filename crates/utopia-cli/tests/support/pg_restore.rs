use std::io::Write;

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["--version"] {
        println!("pg_restore test double");
        return;
    }
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::env::var_os("UTOPIA_RESTORE_TEST_LOG").unwrap())
        .unwrap();
    writeln!(log, "{}", args.join(" ")).unwrap();
    let dump = std::path::Path::new(args.last().expect("pg_restore dump argument"));
    assert!(
        dump.is_file(),
        "pg_restore dump argument must name a file: {}",
        dump.display()
    );
    assert!(
        dump.starts_with(std::env::current_dir().unwrap()),
        "restore staging must stay in current working directory: {}",
        dump.display()
    );
    if let Some(old_file) = std::env::var_os("UTOPIA_RESTORE_TEST_OLD_FILE") {
        assert!(
            std::path::Path::new(&old_file).is_file(),
            "old data was removed before pg_restore"
        );
    }
    if std::env::var_os("UTOPIA_RESTORE_TEST_FAIL").is_some() {
        eprintln!("simulated pg_restore failure");
        std::process::exit(23);
    }
    if let Some(root) = std::env::var_os("UTOPIA_RESTORE_TEST_REMOVE_PREPARED") {
        let root = std::path::Path::new(&root).canonicalize().unwrap();
        for entry in std::fs::read_dir(&root).unwrap() {
            let entry = entry.unwrap();
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(".utopia-restore-data-")
            {
                let prepared = entry.path().join("new").canonicalize().unwrap();
                assert!(
                    prepared.starts_with(&root),
                    "only remove this fixture's staged files"
                );
                std::fs::remove_dir_all(prepared).unwrap();
            }
        }
    }
}
