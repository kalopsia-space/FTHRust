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
    // Force arena allocations and disable glibc's own trimming, so only an
    // explicit trim() can give the freed pages back.
    // SAFETY: mallopt only adjusts allocator parameters.
    unsafe {
        libc::mallopt(libc::M_MMAP_THRESHOLD, 1 << 20);
        libc::mallopt(libc::M_TRIM_THRESHOLD, i32::MAX);
    }
    let threads: Vec<_> = (0..4)
        .map(|_| {
            std::thread::spawn(|| {
                let mut chunks: Vec<Vec<u8>> = (0..800)
                    .map(|_| {
                        let mut v = vec![0u8; 64 * 1024];
                        for i in (0..v.len()).step_by(4096) {
                            v[i] = 1;
                        }
                        v
                    })
                    .collect();
                // Sits above the chunks, so the heap top cannot shrink alone.
                let pin = vec![7u8; 4096];
                chunks.clear();
                drop(chunks);
                pin
            })
        })
        .collect();
    let pins: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    let before = rss_anon_kib();
    alloc::trim();
    let after = rss_anon_kib();
    drop(pins);
    assert!(
        after + 100 * 1024 < before,
        "trim freed too little: {before} KiB -> {after} KiB"
    );
}

#[test]
fn configure_is_idempotent() {
    alloc::configure();
    alloc::configure();
}
