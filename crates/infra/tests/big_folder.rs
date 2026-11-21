//! Performance check on a folder with many files: the tab must not block while it
//! is read, and the listing itself must stay fast. Run with
//! `cargo test -p quokka-infra --release --test big_folder -- --ignored --nocapture`.

use quokka_core::panel::Panel;
use std::time::{Duration, Instant};

#[test]
#[ignore = "creates 50 000 files; run on demand"]
fn fifty_thousand_files() {
    let dir = std::env::temp_dir().join("quokka-big-folder");
    if !dir.join("49999.txt").exists() {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..50_000 {
            std::fs::write(dir.join(format!("{i}.txt")), b"x").unwrap();
        }
    }
    let services = quokka_infra::services(quokka_core::Waker::noop());
    let mut p = Panel::new(std::env::temp_dir(), false, services);
    p.wait_loaded();

    let t0 = Instant::now();
    p.navigate(dir.clone()).unwrap();
    let blocked = t0.elapsed();
    p.wait_loaded();
    let total = t0.elapsed();
    assert_eq!(p.files, 50_000);

    let t1 = Instant::now();
    p.filter = "4999".into();
    p.apply_filter();
    let filter = t1.elapsed();
    // 4999, 14999 … 44999 and 49990 … 49999
    assert_eq!(p.visible.len(), 15);

    let t2 = Instant::now();
    p.set_sort(quokka_domain::SortKey::Size);
    let resort = t2.elapsed();

    println!("UI blocked {blocked:?}, listing {total:?}, filter {filter:?}, re-sort {resort:?}");
    assert!(blocked < Duration::from_millis(500), "navigation blocked the UI for {blocked:?}");
}
