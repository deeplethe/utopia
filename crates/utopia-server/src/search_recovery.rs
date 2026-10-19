//! A document commit is not a completed rebuild. Keep the marker outside the
//! index so recovery state does not depend on the generation being rebuilt.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use utopia_search::SearchIndex;
use uuid::Uuid;

type Row = (Uuid, Uuid, Uuid, String);

fn marker(index_dir: &Path) -> PathBuf {
    index_dir.with_extension("rebuilding")
}

fn sync_parent(path: &Path) -> anyhow::Result<()> {
    // The marker must survive a crash after the first document commits, including
    // its directory entry. Opening directories for syncing is Unix-specific.
    #[cfg(unix)]
    File::open(
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )?
    .sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

pub fn open(index_dir: &Path) -> anyhow::Result<SearchIndex> {
    // Opening first acquires Tantivy's writer lock. A second process must fail
    // here, before it can reset an index the first process is still rebuilding.
    let search = SearchIndex::open(index_dir)?;
    if marker(index_dir).try_exists()? {
        tracing::info!("全文索引重建曾中断，从库里重新开始");
        // Retrying in place would retain documents deleted from the database
        // between attempts. Keep the marker until the fresh run finishes.
        search.clear()?;
    }
    Ok(search)
}

pub fn rebuild(index_dir: &Path, search: &SearchIndex, rows: Vec<Row>) -> anyhow::Result<()> {
    rebuild_with(index_dir, rows, |kb, doc, batch| {
        search.reindex_document(kb, doc, batch)
    })
}

fn rebuild_with(
    index_dir: &Path,
    rows: Vec<Row>,
    mut write: impl FnMut(&str, &str, &[(String, String)]) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let marker = marker(index_dir);
    File::create(&marker)?.sync_all()?;
    sync_parent(&marker)?;

    // The store returns document_id/seq order. Each call replaces a whole
    // document; sending its chunks individually would delete preceding chunks.
    let mut rows = rows.into_iter().peekable();
    while let Some((kb, doc, chunk, text)) = rows.next() {
        let mut batch = vec![(chunk.to_string(), text)];
        while rows.peek().is_some_and(|(_, next, _, _)| *next == doc) {
            let (_, _, chunk, text) = rows.next().expect("peeked row");
            batch.push((chunk.to_string(), text));
        }
        // Propagate a single-document failure: the next boot must retry even
        // when preceding documents have made the index nonempty.
        write(&kb.to_string(), &doc.to_string(), &batch)?;
    }
    fs::remove_file(&marker)?;
    sync_parent(&marker)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(kb: Uuid, docs: [Uuid; 2]) -> Vec<Row> {
        vec![
            (kb, docs[0], Uuid::now_v7(), "firstneedle".into()),
            (kb, docs[0], Uuid::now_v7(), "secondneedle".into()),
            (kb, docs[1], Uuid::now_v7(), "thirdneedle".into()),
        ]
    }

    #[test]
    fn a_restart_retries_an_interrupted_or_failed_rebuild() -> anyhow::Result<()> {
        for interrupted in [true, false] {
            let dir = tempfile::tempdir()?;
            let index_dir = dir.path().join("index");
            let kb = Uuid::now_v7();
            let docs = [Uuid::now_v7(), Uuid::now_v7()];
            let search = open(&index_dir)?;
            let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                rebuild_with(&index_dir, rows(kb, docs), |kb, doc, batch| {
                    if doc == docs[1].to_string() {
                        if interrupted {
                            panic!("interrupted after the first document committed");
                        }
                        anyhow::bail!("one document could not be indexed");
                    }
                    search.reindex_document(kb, doc, batch)
                })
            }));
            assert!(attempt.is_err() || attempt.unwrap().is_err());
            assert_eq!(search.len(), 2, "the first document really committed");
            assert!(marker(&index_dir).exists());
            assert!(open(&index_dir).is_err(), "a second writer must be refused");
            assert_eq!(search.len(), 2, "the live writer keeps its committed data");
            assert_eq!(search.search(&kb.to_string(), "firstneedle", 10)?.len(), 1);
            drop(search);

            let search = open(&index_dir)?;
            assert!(search.is_empty(), "discard the incomplete generation");
            // The first document was deleted between attempts; none of its old
            // chunks may survive, and both chunks of the live document belong.
            let live = vec![
                (kb, docs[1], Uuid::now_v7(), "thirdneedle".into()),
                (kb, docs[1], Uuid::now_v7(), "fourthneedle".into()),
            ];
            rebuild(&index_dir, &search, live)?;
            assert_eq!(search.len(), 2);
            assert!(search
                .search(&kb.to_string(), "firstneedle", 10)?
                .is_empty());
            assert_eq!(search.search(&kb.to_string(), "thirdneedle", 10)?.len(), 1);
            assert!(!marker(&index_dir).exists());
        }
        Ok(())
    }

    #[test]
    fn a_complete_or_legacy_index_is_reused_on_restart() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let index_dir = dir.path().join("index");
        let kb = Uuid::now_v7();
        let docs = [Uuid::now_v7(), Uuid::now_v7()];
        let search = open(&index_dir)?;
        rebuild(&index_dir, &search, rows(kb, docs))?;
        assert!(!marker(&index_dir).exists());
        drop(search);

        let search = open(&index_dir)?;
        assert_eq!(search.len(), 3);
        // A legacy index has no marker either: ordinary writes and the next
        // open must not erase it or turn a normal startup into a rebuild.
        search.reindex_document(
            &kb.to_string(),
            &docs[0].to_string(),
            &[(Uuid::now_v7().to_string(), "legacyneedle".into())],
        )?;
        drop(search);
        let search = open(&index_dir)?;
        assert_eq!(search.len(), 2);
        assert_eq!(search.search(&kb.to_string(), "legacyneedle", 10)?.len(), 1);

        // The database may become empty between attempts. Completing that
        // recovery must clear its marker instead of rebuilding on every boot.
        File::create(marker(&index_dir))?.sync_all()?;
        drop(search);
        let search = open(&index_dir)?;
        assert!(search.is_empty());
        rebuild(&index_dir, &search, Vec::new())?;
        assert!(!marker(&index_dir).exists());
        Ok(())
    }
}
