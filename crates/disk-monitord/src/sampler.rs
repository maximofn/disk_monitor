use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use disk_monitor_core::Snapshot;
use tokio::sync::watch;
use tokio::time::{interval, MissedTickBehavior};

use crate::disk_source::DiskSource;
use crate::largest_files::LargestFilesCache;

pub fn empty_snapshot(host: &str) -> Snapshot {
    Snapshot {
        timestamp: Utc::now().to_rfc3339(),
        host: host.to_string(),
        mounts: Vec::new(),
    }
}

pub fn build_snapshot(
    host: &str,
    source: &dyn DiskSource,
    cache: &LargestFilesCache,
) -> Snapshot {
    let mut mounts = source.sample().unwrap_or_else(|err| {
        tracing::warn!(error = %err, "disk sample failed; emitting empty list");
        Vec::new()
    });
    for m in &mut mounts {
        if let Some(scan) = cache.get(&m.mount_point) {
            m.largest_files = scan.files;
            m.largest_files_scanned_at = Some(scan.scanned_at);
        }
    }
    Snapshot {
        timestamp: Utc::now().to_rfc3339(),
        host: host.to_string(),
        mounts,
    }
}

pub fn spawn(
    source: Arc<dyn DiskSource>,
    host: String,
    interval_ms: u64,
    cache: LargestFilesCache,
    tx: watch::Sender<Snapshot>,
) {
    tokio::spawn(async move {
        let period = Duration::from_millis(interval_ms.max(50));
        let mut ticker = interval(period);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            ticker.tick().await;
            let snapshot = build_snapshot(&host, source.as_ref(), &cache);
            if tx.send(snapshot).is_err() {
                tracing::info!("snapshot channel closed; sampler exiting");
                break;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk_source::MockSource;
    use disk_monitor_core::{Mount, Usage};

    fn fake_mount(p: &str) -> Mount {
        Mount {
            mount_point: p.into(),
            device: format!("/dev/fake{}", p.replace('/', "_")),
            fs_type: "ext4".into(),
            usage: Usage {
                used_bytes: 100,
                free_bytes: 900,
                total_bytes: 1000,
            },
            largest_files: Vec::new(),
            largest_files_scanned_at: None,
        }
    }

    #[test]
    fn build_snapshot_uses_source_data() {
        let source = MockSource::new(vec![fake_mount("/"), fake_mount("/home")]);
        let cache = LargestFilesCache::new();
        let snap = build_snapshot("host-x", &source, &cache);
        assert_eq!(snap.host, "host-x");
        assert_eq!(snap.mounts.len(), 2);
        assert!(!snap.timestamp.is_empty());
        assert!(snap.mounts[0].largest_files.is_empty());
    }

    #[test]
    fn build_snapshot_merges_scan_cache() {
        use crate::largest_files::ScanResult;
        use disk_monitor_core::FileEntry;
        let source = MockSource::new(vec![fake_mount("/")]);
        let cache = LargestFilesCache::new();
        cache.put(
            "/".into(),
            ScanResult {
                files: vec![FileEntry {
                    path: "/big".into(),
                    size_bytes: 999,
                }],
                scanned_at: "ts".into(),
            },
        );
        let snap = build_snapshot("h", &source, &cache);
        assert_eq!(snap.mounts[0].largest_files.len(), 1);
        assert_eq!(snap.mounts[0].largest_files[0].path, "/big");
        assert_eq!(
            snap.mounts[0].largest_files_scanned_at.as_deref(),
            Some("ts")
        );
    }

    #[test]
    fn empty_snapshot_has_no_mounts() {
        let snap = empty_snapshot("h");
        assert!(snap.mounts.is_empty());
    }
}
