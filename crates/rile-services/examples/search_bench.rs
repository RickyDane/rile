//! Times a name search and a content search over a directory.
//!
//! Usage: cargo run --release -p rile-services --example search_bench -- <dir> [name] [content]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use rile_services::search::{ContentMatch, NameMatch, SearchQuery, search};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let dir = args.next().unwrap_or_else(|| ".".into());
    let name = args.next().unwrap_or_else(|| "rs".into());
    let content = args.next().unwrap_or_else(|| "fn main".into());
    let cancel = AtomicBool::new(false);

    let run = |label: &str, q: &SearchQuery| -> anyhow::Result<()> {
        let hits = AtomicU64::new(0);
        let s = search(q, &cancel, |_| {
            hits.fetch_add(1, Ordering::Relaxed);
        })?;
        println!(
            "{label:<38} scanned {:>8}  matched {:>7}  in {:>8.2?}",
            s.scanned, s.matched, s.elapsed
        );
        Ok(())
    };

    for (hidden, gitignore) in [(false, true), (true, false)] {
        let mut q = SearchQuery::new(&dir);
        q.max_results = 0;
        q.include_hidden = hidden;
        q.respect_gitignore = gitignore;
        let tag = if gitignore { "[gitignore]" } else { "[all files]" };

        q.name = NameMatch::Substring(String::new());
        run(&format!("{tag} walk everything"), &q)?;
        q.name = NameMatch::Substring(name.clone());
        run(&format!("{tag} name contains {name:?}"), &q)?;
        q.content = Some(ContentMatch { pattern: content.clone(), regex: false, case_sensitive: false });
        run(&format!("{tag} name + content {content:?}"), &q)?;
        q.name = NameMatch::Substring(String::new());
        run(&format!("{tag} content {content:?}"), &q)?;
    }
    Ok(())
}
