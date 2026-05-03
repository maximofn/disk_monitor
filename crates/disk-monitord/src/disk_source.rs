use std::fs;
use std::path::Path;

use anyhow::Result;
use disk_monitor_core::{Mount, Usage};
use nix::sys::statvfs::statvfs;

pub trait DiskSource: Send + Sync {
    fn sample(&self) -> Result<Vec<Mount>>;
}

/// Real disk source: reads `/proc/mounts` and calls `statvfs` for each
/// non-pseudo filesystem.
pub struct ProcfsSource;

impl ProcfsSource {
    pub fn new() -> Self {
        Self
    }
}

impl DiskSource for ProcfsSource {
    fn sample(&self) -> Result<Vec<Mount>> {
        let raw = fs::read_to_string("/proc/mounts")?;
        let mut out = Vec::new();
        for line in raw.lines() {
            let entry = match parse_mount_line(line) {
                Some(e) => e,
                None => continue,
            };
            if !is_real_filesystem(&entry) {
                continue;
            }
            // Avoid sampling the same backing device twice (e.g. bind mounts).
            if out
                .iter()
                .any(|m: &Mount| m.device == entry.device && entry.device.starts_with("/dev/"))
            {
                continue;
            }
            match read_usage(&entry.mount_point) {
                Ok(usage) => out.push(Mount {
                    mount_point: entry.mount_point,
                    device: entry.device,
                    fs_type: entry.fs_type,
                    usage,
                    largest_files: Vec::new(),
                    largest_files_scanned_at: None,
                }),
                Err(err) => {
                    tracing::debug!(mount = %entry.mount_point, error = %err, "statvfs failed");
                }
            }
        }
        // Stable ordering: root first, then alphabetical.
        out.sort_by(|a, b| match (a.mount_point.as_str(), b.mount_point.as_str()) {
            ("/", _) => std::cmp::Ordering::Less,
            (_, "/") => std::cmp::Ordering::Greater,
            (x, y) => x.cmp(y),
        });
        Ok(out)
    }
}

struct ParsedEntry {
    device: String,
    mount_point: String,
    fs_type: String,
}

fn parse_mount_line(line: &str) -> Option<ParsedEntry> {
    let mut parts = line.split_whitespace();
    let device = parts.next()?;
    let mount_point = parts.next()?;
    let fs_type = parts.next()?;
    Some(ParsedEntry {
        device: unescape_octal(device),
        mount_point: unescape_octal(mount_point),
        fs_type: fs_type.to_string(),
    })
}

/// `/proc/mounts` escapes spaces and a few other chars as `\040`-style octals.
fn unescape_octal(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            let triplet = &bytes[i + 1..i + 4];
            if triplet.iter().all(|b| (b'0'..=b'7').contains(b)) {
                let mut value: u8 = 0;
                for b in triplet {
                    value = value * 8 + (b - b'0');
                }
                out.push(value);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

const PSEUDO_FILESYSTEMS: &[&str] = &[
    "proc",
    "sysfs",
    "tmpfs",
    "devtmpfs",
    "devpts",
    "cgroup",
    "cgroup2",
    "pstore",
    "bpf",
    "tracefs",
    "debugfs",
    "securityfs",
    "configfs",
    "fusectl",
    "fuse.gvfsd-fuse",
    "fuse.portal",
    "mqueue",
    "hugetlbfs",
    "autofs",
    "binfmt_misc",
    "rpc_pipefs",
    "nsfs",
    "ramfs",
    "squashfs",
    "overlay",
    "efivarfs",
    "selinuxfs",
];

fn is_real_filesystem(entry: &ParsedEntry) -> bool {
    if PSEUDO_FILESYSTEMS.contains(&entry.fs_type.as_str()) {
        return false;
    }
    // Snap mounts are squashfs but the type filter already covers them; here we
    // also drop anything mounted under /snap/ explicitly to be safe.
    if entry.mount_point.starts_with("/snap/") {
        return false;
    }
    if entry.mount_point.starts_with("/var/snap/") {
        return false;
    }
    if entry.mount_point.starts_with("/proc")
        || entry.mount_point.starts_with("/sys")
        || entry.mount_point.starts_with("/dev")
        || entry.mount_point.starts_with("/run")
    {
        return false;
    }
    // EFI System Partition: vfat mounted under /boot/. Always tiny (~500 MiB),
    // managed by the bootloader, not actionable for the user.
    if entry.fs_type == "vfat" && entry.mount_point.starts_with("/boot") {
        return false;
    }
    true
}

fn read_usage(mount_point: &str) -> Result<Usage> {
    let stat = statvfs(Path::new(mount_point))?;
    // Use f_frsize (fundamental block size) for total/free, NOT f_bsize.
    let frsize = stat.fragment_size() as u64;
    let total_bytes = stat.blocks() as u64 * frsize;
    // f_bavail = blocks available to non-root processes; matches what `df`
    // and Python's `shutil.disk_usage` report as "free".
    let free_bytes = stat.blocks_available() as u64 * frsize;
    let used_bytes = total_bytes.saturating_sub(free_bytes);
    Ok(Usage {
        used_bytes,
        free_bytes,
        total_bytes,
    })
}

pub struct MockSource {
    mounts: Vec<Mount>,
}

impl MockSource {
    pub fn new(mounts: Vec<Mount>) -> Self {
        Self { mounts }
    }
}

impl DiskSource for MockSource {
    fn sample(&self) -> Result<Vec<Mount>> {
        Ok(self.mounts.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pseudo_filesystems_are_filtered() {
        let line = "tmpfs /run tmpfs rw,nosuid 0 0";
        let parsed = parse_mount_line(line).unwrap();
        assert!(!is_real_filesystem(&parsed));
    }

    #[test]
    fn root_ext4_is_kept() {
        let line = "/dev/nvme0n1p2 / ext4 rw,relatime 0 0";
        let parsed = parse_mount_line(line).unwrap();
        assert!(is_real_filesystem(&parsed));
        assert_eq!(parsed.device, "/dev/nvme0n1p2");
        assert_eq!(parsed.mount_point, "/");
    }

    #[test]
    fn efi_partition_is_filtered() {
        let line = "/dev/nvme0n1p1 /boot/efi vfat rw,relatime 0 0";
        let parsed = parse_mount_line(line).unwrap();
        assert!(!is_real_filesystem(&parsed));
    }

    #[test]
    fn snap_mounts_are_filtered() {
        let line = "/dev/loop0 /snap/core/12345 squashfs ro,nodev 0 0";
        let parsed = parse_mount_line(line).unwrap();
        assert!(!is_real_filesystem(&parsed));
    }

    #[test]
    fn mount_point_with_space_decodes_octal() {
        let line = "/dev/sdb1 /media/My\\040Backup ext4 rw 0 0";
        let parsed = parse_mount_line(line).unwrap();
        assert_eq!(parsed.mount_point, "/media/My Backup");
    }

    #[test]
    fn mock_source_returns_seeded_mounts() {
        let mount = Mount {
            mount_point: "/".into(),
            device: "/dev/sda1".into(),
            fs_type: "ext4".into(),
            usage: Usage {
                used_bytes: 50,
                free_bytes: 50,
                total_bytes: 100,
            },
            largest_files: Vec::new(),
            largest_files_scanned_at: None,
        };
        let mock = MockSource::new(vec![mount.clone()]);
        let sample = mock.sample().unwrap();
        assert_eq!(sample.len(), 1);
        assert_eq!(sample[0], mount);
    }
}
