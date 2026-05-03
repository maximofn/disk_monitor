use std::net::IpAddr;

use clap::Parser;
use disk_monitor_core::{DEFAULT_BIND, DEFAULT_PORT};

#[derive(Debug, Clone, Parser)]
#[command(name = "disk-monitord", about = "Disk monitor backend daemon", version)]
pub struct Config {
    #[arg(long, env = "DISK_MONITORD_BIND", default_value = DEFAULT_BIND)]
    pub bind: IpAddr,

    #[arg(long, env = "DISK_MONITORD_PORT", default_value_t = DEFAULT_PORT)]
    pub port: u16,

    #[arg(long, env = "DISK_MONITORD_SAMPLE_INTERVAL_MS", default_value_t = 1000)]
    pub sample_interval_ms: u64,

    #[arg(long, env = "RUST_LOG", default_value = "info")]
    pub log_level: String,

    #[arg(long, env = "DISK_MONITORD_MOCK", default_value_t = false)]
    pub mock: bool,

    /// How many largest files to keep per mount.
    #[arg(long, env = "DISK_MONITORD_LARGEST_TOP_N", default_value_t = 20)]
    pub largest_top_n: usize,

    /// Seconds between full largest-files re-scans. Re-scans are also
    /// triggered on demand via `POST /v1/rescan` regardless of this interval.
    #[arg(long, env = "DISK_MONITORD_LARGEST_REFRESH_SECS", default_value_t = 300)]
    pub largest_refresh_secs: u64,

    /// Seconds to wait before the first largest-files scan kicks in.
    /// Avoids competing with login I/O when the daemon starts.
    #[arg(long, env = "DISK_MONITORD_LARGEST_INITIAL_DELAY_SECS", default_value_t = 30)]
    pub largest_initial_delay_secs: u64,

    /// Disable the largest-files background scanner entirely.
    #[arg(long, env = "DISK_MONITORD_NO_LARGEST_FILES", default_value_t = false)]
    pub no_largest_files: bool,
}
