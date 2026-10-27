//! Times listing + sorting a directory: `cargo run --release -p rile-core --example bench_list -- <dir>`
use std::time::Instant;

fn main() {
    let dir = std::env::args().nth(1).expect("usage: bench_list <dir>");
    for _ in 0..3 {
        let t = Instant::now();
        let mut entries = rile_core::list_dir(dir.as_ref(), true).unwrap();
        let listed = t.elapsed();
        rile_core::sort_entries(&mut entries, rile_core::SortKey::Name, rile_core::SortOrder::Ascending);
        println!("{} entries: list {:?}, total {:?}", entries.len(), listed, t.elapsed());
    }
}
