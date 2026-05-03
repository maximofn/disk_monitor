use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use disk_monitor_core::{FileEntry, Snapshot};
use tokio::sync::{mpsc, watch};
use walkdir::WalkDir;

#[derive(Clone, Debug)]
pub struct ScanResult {
    pub files: Vec<FileEntry>,
    pub scanned_at: String,
}

/// In-memory cache mapping mount_point → most recent scan result.
/// Sampler reads it when assembling each snapshot; scanner writes it after
/// every full walk completes.
#[derive(Clone, Default)]
pub struct LargestFilesCache {
    inner: Arc<Mutex<HashMap<String, ScanResult>>>,
}

impl LargestFilesCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, mount_point: &str) -> Option<ScanResult> {
        self.inner.lock().unwrap().get(mount_point).cloned()
    }

    pub fn put(&self, mount_point: String, result: ScanResult) {
        self.inner.lock().unwrap().insert(mount_point, result);
    }

    /// Drop entries for mounts that no longer appear in `keep`.
    pub fn retain(&self, keep: &[String]) {
        let mut g = self.inner.lock().unwrap();
        g.retain(|k, _| keep.contains(k));
    }
}

/// Request for the scanner: scan a specific mount immediately, or all of
/// them. The HTTP layer sends these via `RescanTrigger`.
#[derive(Clone, Debug)]
pub enum RescanRequest {
    All,
    One(String),
}

pub type RescanTrigger = mpsc::UnboundedSender<RescanRequest>;

#[derive(Clone, Debug)]
pub struct ScannerConfig {
    pub top_n: usize,
    pub refresh_interval: Duration,
    pub initial_delay: Duration,
}

impl Default for ScannerConfig {
    fn default() -> Self {
        Self {
            top_n: 20,
            refresh_interval: Duration::from_secs(300),
            initial_delay: Duration::from_secs(30),
        }
    }
}

/// Spawn the background largest-files scanner. It scans every mount on a
/// timer and on demand whenever a `RescanRequest` arrives. Walks happen on
/// the blocking pool so the runtime stays responsive.
///
/// Returns an mpsc Sender used to trigger ad-hoc rescans (e.g. after the
/// frontend deletes a file, the rescan reflects the new top-N).
pub fn spawn(
    cfg: ScannerConfig,
    cache: LargestFilesCache,
    snapshot_rx: watch::Receiver<Snapshot>,
) -> RescanTrigger {
    let (trigger_tx, mut trigger_rx) = mpsc::unbounded_channel::<RescanRequest>();

    tokio::spawn(async move {
        // Initial scan of every mount, after a delay.
        tokio::time::sleep(cfg.initial_delay).await;
        scan_all(&cfg, &cache, &snapshot_rx).await;

        loop {
            tokio::select! {
                _ = tokio::time::sleep(cfg.refresh_interval) => {
                    scan_all(&cfg, &cache, &snapshot_rx).await;
                }
                Some(req) = trigger_rx.recv() => {
                    // Drain anything else that's queued up to coalesce bursts
                    // (e.g. the user trashing 5 files in quick succession).
                    let mut targets: HashSet<String> = HashSet::new();
                    let mut all = false;
                    let handle = |r: RescanRequest, targets: &mut HashSet<String>, all: &mut bool| match r {
                        RescanRequest::All => *all = true,
                        RescanRequest::One(mp) => { targets.insert(mp); }
                    };
                    handle(req, &mut targets, &mut all);
                    while let Ok(extra) = trigger_rx.try_recv() {
                        handle(extra, &mut targets, &mut all);
                    }
                    if all {
                        scan_all(&cfg, &cache, &snapshot_rx).await;
                    } else {
                        for mp in targets {
                            scan_one(&cfg, &cache, &mp).await;
                        }
                    }
                }
            }
        }
    });

    trigger_tx
}

async fn scan_all(cfg: &ScannerConfig, cache: &LargestFilesCache, rx: &watch::Receiver<Snapshot>) {
    let mounts: Vec<String> = rx.borrow().mounts.iter().map(|m| m.mount_point.clone()).collect();
    cache.retain(&mounts);
    for mp in mounts {
        scan_one(cfg, cache, &mp).await;
    }
}

async fn scan_one(cfg: &ScannerConfig, cache: &LargestFilesCache, mount_point: &str) {
    let cache = cache.clone();
    let mp = mount_point.to_string();
    let top_n = cfg.top_n;
    let result = tokio::task::spawn_blocking(move || {
        let started = Instant::now();
        let files = scan_mount(Path::new(&mp), top_n);
        let elapsed = started.elapsed();
        tracing::info!(
            mount = %mp,
            files = files.len(),
            elapsed_ms = elapsed.as_millis() as u64,
            "largest-files scan complete"
        );
        ScanResult {
            files,
            scanned_at: Utc::now().to_rfc3339(),
        }
    })
    .await;
    match result {
        Ok(res) => cache.put(mount_point.to_string(), res),
        Err(err) => tracing::warn!(mount = %mount_point, error = %err, "scan task panicked"),
    }
}

/// Walk `mount_point` and return the `top_n` largest regular files.
///
/// - Stays within one filesystem (does not descend into nested mounts).
/// - Skips symlinks (does not follow them).
/// - Dedupes hardlinks via (dev, inode) so a file with N hardlinks counts once.
/// - Silently swallows permission and read errors (matches what `du` and
///   `baobab` do when run unprivileged).
pub fn scan_mount(mount_point: &Path, top_n: usize) -> Vec<FileEntry> {
    if top_n == 0 {
        return Vec::new();
    }
    // Min-heap by size: when new size > heap.peek().size, pop and push.
    // Reverse() flips BinaryHeap (max-heap by default) into a min-heap.
    let mut heap: BinaryHeap<Reverse<(u64, PathBuf)>> = BinaryHeap::with_capacity(top_n + 1);
    let mut seen_inodes: HashSet<(u64, u64)> = HashSet::new();

    let walker = WalkDir::new(mount_point)
        .same_file_system(true)
        .follow_links(false);

    for entry in walker.into_iter().filter_map(|r| r.ok()) {
        let ft = entry.file_type();
        if !ft.is_file() {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        // Dedupe hardlinks: same (dev, inode) only once, regardless of which
        // path we encountered first. Lower path is kept by walkdir order.
        if meta.nlink() > 1 && !seen_inodes.insert((meta.dev(), meta.ino())) {
            continue;
        }
        let size = meta.len();
        if heap.len() < top_n {
            heap.push(Reverse((size, entry.path().to_path_buf())));
        } else if let Some(Reverse((min_size, _))) = heap.peek() {
            if size > *min_size {
                heap.pop();
                heap.push(Reverse((size, entry.path().to_path_buf())));
            }
        }
    }

    let mut out: Vec<(u64, PathBuf)> = heap.into_iter().map(|r| r.0).collect();
    out.sort_by_key(|p| Reverse(p.0));
    out.into_iter()
        .map(|(size, path)| FileEntry {
            path: path.to_string_lossy().into_owned(),
            size_bytes: size,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn scan_returns_top_n_in_size_order() {
        let dir = tempdir().unwrap();
        for (name, size) in [("a", 100u64), ("b", 5_000), ("c", 1_000), ("d", 50)] {
            let mut f = fs::File::create(dir.path().join(name)).unwrap();
            f.write_all(&vec![0u8; size as usize]).unwrap();
        }
        let result = scan_mount(dir.path(), 2);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].size_bytes, 5_000);
        assert!(result[0].path.ends_with("/b"));
        assert_eq!(result[1].size_bytes, 1_000);
        assert!(result[1].path.ends_with("/c"));
    }

    #[test]
    fn scan_returns_all_files_when_fewer_than_top_n() {
        let dir = tempdir().unwrap();
        for (name, size) in [("a", 100u64), ("b", 200)] {
            fs::File::create(dir.path().join(name))
                .unwrap()
                .write_all(&vec![0u8; size as usize])
                .unwrap();
        }
        let result = scan_mount(dir.path(), 10);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn scan_skips_symlinks() {
        let dir = tempdir().unwrap();
        fs::File::create(dir.path().join("real"))
            .unwrap()
            .write_all(&vec![0u8; 1000])
            .unwrap();
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("link")).unwrap();
        let result = scan_mount(dir.path(), 10);
        assert_eq!(result.len(), 1);
        assert!(result[0].path.ends_with("/real"));
    }

    #[test]
    fn scan_skips_hardlinks_after_first() {
        let dir = tempdir().unwrap();
        let real = dir.path().join("real");
        fs::File::create(&real)
            .unwrap()
            .write_all(&vec![0u8; 1000])
            .unwrap();
        fs::hard_link(&real, dir.path().join("link")).unwrap();
        let result = scan_mount(dir.path(), 10);
        assert_eq!(result.len(), 1, "hardlinked twin should be dropped");
    }

    #[test]
    fn cache_round_trip_and_retain() {
        let cache = LargestFilesCache::new();
        cache.put(
            "/home".into(),
            ScanResult {
                files: vec![],
                scanned_at: "now".into(),
            },
        );
        cache.put(
            "/data".into(),
            ScanResult {
                files: vec![],
                scanned_at: "now".into(),
            },
        );
        assert!(cache.get("/home").is_some());
        cache.retain(&["/data".into()]);
        assert!(cache.get("/home").is_none());
        assert!(cache.get("/data").is_some());
    }
}
