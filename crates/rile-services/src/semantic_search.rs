//! AI chooses subject folders; ordinary search retrieves their contents.
//! Model context limits must never become file-result limits.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Mutex, atomic::{AtomicBool, Ordering}};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use rile_core::Entry;

use crate::search::{self, KindFilter, SearchQuery};

const FOLDER_BATCH_SIZE: usize = 20;

/// `choose` returns one-based indices of folders whose contents match the
/// subject, not generic places that might contain a match. Results stream as
/// each batch is reviewed: cached entries first, then a recursive disk search.
/// The query supplies type/date/visibility filters and the user's result limit;
/// its name/content filters must be empty (the subject was matched by folder).
pub fn search_folders(
    folders: &[PathBuf],
    query: &SearchQuery,
    cancel: &AtomicBool,
    choose: impl FnMut(&[PathBuf]) -> Result<Vec<usize>>,
    push: &(dyn Fn(Vec<Entry>) + Sync),
) -> Result<usize> {
    search_folders_with_index(folders, query, cancel, choose, crate::index::query, push)
}

fn search_folders_with_index(
    folders: &[PathBuf],
    query: &SearchQuery,
    cancel: &AtomicBool,
    mut choose: impl FnMut(&[PathBuf]) -> Result<Vec<usize>>,
    remembered: impl Fn(&SearchQuery) -> Vec<Entry>,
    push: &(dyn Fn(Vec<Entry>) + Sync),
) -> Result<usize> {
    search::validate(query)?;
    let state = Mutex::new((HashSet::new(), Vec::new(), Instant::now()));
    let emit = |entry: Entry| {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let mut s = state.lock().unwrap();
        if (query.max_results != 0 && s.0.len() >= query.max_results) || !s.0.insert(entry.path.clone()) {
            return;
        }
        s.1.push(entry);
        if s.1.len() >= 256 || s.2.elapsed() >= Duration::from_millis(50) {
            push(std::mem::take(&mut s.1));
            s.2 = Instant::now();
        }
    };
    let flush = || {
        let mut s = state.lock().unwrap();
        if !s.1.is_empty() {
            push(std::mem::take(&mut s.1));
            s.2 = Instant::now();
        }
    };
    let mut expanded: Vec<PathBuf> = Vec::new();
    for batch in folders.chunks(FOLDER_BATCH_SIZE) {
        if cancel.load(Ordering::Relaxed) {
            bail!("Cancelled");
        }
        if query.max_results != 0 && state.lock().unwrap().0.len() >= query.max_results {
            break;
        }
        let picks = choose(batch)?;
        if cancel.load(Ordering::Relaxed) {
            bail!("Cancelled");
        }
        for pick in picks {
            if cancel.load(Ordering::Relaxed) {
                bail!("Cancelled");
            }
            if query.max_results != 0 && state.lock().unwrap().0.len() >= query.max_results {
                break;
            }
            let Some(path) = pick.checked_sub(1).and_then(|i| batch.get(i)) else { continue };
            if expanded.iter().any(|root| path.starts_with(root)) {
                continue;
            }
            let Some(folder) = Entry::from_path(path.clone()).filter(|e| e.is_dir && (query.include_hidden || !e.hidden)) else { continue };
            if query.kind != KindFilter::Files && query.extensions.is_empty() && search::meta_ok(query, &folder) {
                emit(folder);
            }
            let mut q = query.clone();
            q.root = path.clone();
            // Include enough room for cached/overlapping hits; deduplication
            // and the overall user limit are enforced at emission, not by AI.
            if q.max_results != 0 {
                q.max_results = q.max_results.saturating_add(state.lock().unwrap().0.len());
            }
            for entry in remembered(&q) {
                emit(entry);
            }
            flush(); // Never wait for disk traversal before showing memory.
            if query.max_results == 0 || state.lock().unwrap().0.len() < query.max_results {
                search::search(&q, cancel, |hit| emit(hit.entry))?;
            }
            flush();
            expanded.push(path.clone());
        }
    }
    if cancel.load(Ordering::Relaxed) {
        bail!("Cancelled");
    }
    flush();
    Ok(state.into_inner().unwrap().0.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::DiscoveryIndex;

    #[test]
    fn streams_all_invoices_including_nested_and_unremembered_files() {
        let tmp = tempfile::tempdir().unwrap();
        let outgoing = tmp.path().join("Ausgangsrechnungen");
        std::fs::create_dir_all(outgoing.join("2025")).unwrap();
        for i in 0..125 {
            std::fs::write(outgoing.join(format!("{i:03}.pdf")), "invoice").unwrap();
        }
        let mut index = DiscoveryIndex::default();
        index.record(&outgoing, &rile_core::list_dir(&outgoing, false).unwrap());
        std::fs::write(outgoing.join("2025/uuid.pdf"), "invoice").unwrap();
        std::fs::write(outgoing.join("new.pdf"), "invoice").unwrap();
        std::fs::write(outgoing.join(".hidden.pdf"), "invoice").unwrap();
        let batches = Mutex::new(Vec::new());
        let query = SearchQuery::new(tmp.path());
        let count = search_folders_with_index(
            &[outgoing.clone(), outgoing.join("2025")], &query, &AtomicBool::new(false),
            |_| Ok(vec![1, 2, 1, 0, 999]), |q| index.query(q),
            &|batch| batches.lock().unwrap().push(batch),
        ).unwrap();
        let batches = batches.into_inner().unwrap();
        assert!(batches.len() >= 2);
        assert!(batches[0].iter().any(|e| e.path == outgoing));
        assert!(!batches[0].iter().any(|e| e.name.as_ref() == "new.pdf"));
        let entries: Vec<_> = batches.into_iter().flatten().collect();
        assert_eq!(count, 129); // root + year + 127 PDFs
        assert_eq!(entries.len(), count);
        assert_eq!(entries.iter().map(|e| &e.path).collect::<HashSet<_>>().len(), count);
        assert!(entries.iter().any(|e| e.name.as_ref() == "uuid.pdf"));
    }

    #[test]
    fn reviews_later_batches_even_after_a_match_and_streams_before_next_review() {
        let tmp = tempfile::tempdir().unwrap();
        let folders: Vec<_> = (0..25).map(|i| {
            let p = tmp.path().join(format!("outgoing-{i}"));
            std::fs::create_dir(&p).unwrap();
            std::fs::write(p.join("uuid.pdf"), "").unwrap();
            p
        }).collect();
        let entries = Mutex::new(Vec::new());
        let mut calls = 0;
        let count = search_folders_with_index(&folders, &SearchQuery::new(tmp.path()), &AtomicBool::new(false), |batch| {
            calls += 1;
            if calls == 2 {
                assert_eq!(entries.lock().unwrap().len(), 40);
            }
            Ok((1..=batch.len()).collect())
        }, |_| Vec::new(), &|batch| entries.lock().unwrap().extend(batch)).unwrap();
        assert_eq!(calls, 2);
        assert_eq!(count, 50);
    }

    #[test]
    fn honors_filters_limits_stale_paths_and_cancellation() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("invoices");
        std::fs::create_dir(&folder).unwrap();
        for name in ["a.pdf", "b.pdf", "c.txt", ".hidden.pdf"] {
            std::fs::write(folder.join(name), "").unwrap();
        }
        let mut query = SearchQuery::new(tmp.path());
        query.kind = KindFilter::Files;
        query.extensions = vec!["pdf".into()];
        query.max_results = 1;
        let entries = Mutex::new(Vec::new());
        let paths = [tmp.path().join("deleted"), folder.clone(), folder.join("a.pdf")];
        assert_eq!(search_folders_with_index(&paths, &query, &AtomicBool::new(false), |_| Ok(vec![1, 2, 3]), |_| Vec::new(), &|batch| entries.lock().unwrap().extend(batch)).unwrap(), 1);
        assert!(entries.lock().unwrap().iter().all(|e| !e.is_dir && e.extension() == "pdf" && !e.hidden));
        query.modified_after = Some(std::time::SystemTime::now() + Duration::from_secs(3600));
        assert_eq!(search_folders_with_index(&paths, &query, &AtomicBool::new(false), |_| Ok(vec![2]), |_| Vec::new(), &|_| panic!("date filter ignored")).unwrap(), 0);
        let cancel = AtomicBool::new(true);
        assert!(search_folders_with_index(&paths, &query, &cancel, |_| panic!("model called after cancellation"), |_| Vec::new(), &|_| {}).is_err());
        cancel.store(false, Ordering::Relaxed);
        assert!(search_folders_with_index(&paths, &query, &cancel, |_| { cancel.store(true, Ordering::Relaxed); Ok(vec![2]) }, |_| Vec::new(), &|_| panic!("emitted after cancellation")).is_err());
    }

    #[test]
    fn does_not_expand_unselected_incoming_folders_or_stale_cached_files() {
        let tmp = tempfile::tempdir().unwrap();
        let outgoing = tmp.path().join("Ausgangsrechnungen");
        let incoming = tmp.path().join("Eingangsrechnungen");
        for folder in [&outgoing, &incoming] {
            std::fs::create_dir(folder).unwrap();
            std::fs::write(folder.join("uuid.pdf"), "").unwrap();
        }
        std::fs::write(outgoing.join("deleted.pdf"), "").unwrap();
        let mut index = DiscoveryIndex::default();
        index.record(&outgoing, &rile_core::list_dir(&outgoing, false).unwrap());
        std::fs::remove_file(outgoing.join("deleted.pdf")).unwrap();
        let entries = Mutex::new(Vec::new());
        let mut query = SearchQuery::new(tmp.path());
        query.max_results = 0;
        let folders = [outgoing.clone(), incoming];
        assert_eq!(search_folders_with_index(&folders, &query, &AtomicBool::new(false), |_| Ok(vec![1]), |q| index.query(q), &|batch| entries.lock().unwrap().extend(batch)).unwrap(), 2);
        assert!(entries.lock().unwrap().iter().all(|e| e.path.starts_with(&outgoing)));
        assert_eq!(search_folders_with_index(&folders, &query, &AtomicBool::new(false), |_| Ok(vec![]), |_| panic!("read unselected folder"), &|_| panic!("emitted without a subject match")).unwrap(), 0);
    }
}
