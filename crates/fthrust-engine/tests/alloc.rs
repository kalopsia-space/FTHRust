use fthrust_engine::alloc;

fn rss_anon_kib() -> u64 {
    let s = std::fs::read_to_string("/proc/self/status").unwrap();
    s.lines()
        .find_map(|l| l.strip_prefix("RssAnon:"))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
        .unwrap()
}

#[test]
fn trim_returns_freed_memory() {
    let before = rss_anon_kib();
    let threads: Vec<_> = (0..4)
        .map(|_| {
            std::thread::spawn(|| {
                for _ in 0..50 {
                    let mut v = vec![0u8; 1 << 20];
                    for i in (0..v.len()).step_by(4096) {
                        v[i] = 1;
                    }
                    std::hint::black_box(&v);
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    alloc::trim();
    let after = rss_anon_kib();
    assert!(
        after <= before + 64 * 1024,
        "RssAnon grew from {before} KiB to {after} KiB"
    );
}

#[test]
fn configure_is_idempotent() {
    alloc::configure();
    alloc::configure();
}
