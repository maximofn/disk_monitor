mod config;
mod disk_source;
mod http;
mod largest_files;
mod sampler;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use config::Config;
use disk_monitor_core::Snapshot;
use disk_source::{DiskSource, MockSource, ProcfsSource};
use largest_files::{LargestFilesCache, ScannerConfig};
use sampler::{build_snapshot, empty_snapshot};
use tokio::net::TcpListener;
use tokio::signal;
use tokio::sync::watch;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    let cfg = Config::parse();
    init_tracing(&cfg.log_level);

    let host = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "localhost".to_string());

    let source: Arc<dyn DiskSource> = if cfg.mock {
        tracing::warn!("running with MOCK disk source");
        Arc::new(MockSource::new(mock_mounts()))
    } else {
        Arc::new(ProcfsSource::new())
    };

    let cache = LargestFilesCache::new();

    let initial: Snapshot = match source.sample() {
        Ok(_) => build_snapshot(&host, source.as_ref(), &cache),
        Err(err) => {
            tracing::warn!(error = %err, "initial sample failed; serving empty snapshot");
            empty_snapshot(&host)
        }
    };
    let (tx, rx) = watch::channel(initial);

    sampler::spawn(source, host, cfg.sample_interval_ms, cache.clone(), tx);

    let rescan_tx = if !cfg.no_largest_files {
        let scanner_cfg = ScannerConfig {
            top_n: cfg.largest_top_n,
            refresh_interval: Duration::from_secs(cfg.largest_refresh_secs),
            initial_delay: Duration::from_secs(cfg.largest_initial_delay_secs),
        };
        tracing::info!(
            top_n = scanner_cfg.top_n,
            refresh_secs = cfg.largest_refresh_secs,
            initial_delay_secs = cfg.largest_initial_delay_secs,
            "spawning largest-files scanner"
        );
        Some(largest_files::spawn(scanner_cfg, cache, rx.clone()))
    } else {
        tracing::info!("largest-files scanner disabled by --no-largest-files");
        None
    };

    let state = http::AppState {
        started_at: Instant::now(),
        snapshot_rx: rx,
        rescan_tx,
    };
    let app = http::build_router(state);

    let addr = SocketAddr::new(cfg.bind, cfg.port);
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;
    tracing::info!(%addr, "disk-monitord listening");

    tokio::select! {
        result = axum::serve(listener, app) => {
            result.context("HTTP server error")?;
        }
        _ = shutdown_signal() => {
            tracing::info!("shutdown requested; aborting in-flight SSE streams");
        }
    }

    tracing::info!("shutdown complete");
    Ok(())
}

fn init_tracing(directive: &str) {
    let filter = EnvFilter::try_new(directive).unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c().await.ok();
    };

    #[cfg(unix)]
    let terminate = async {
        match signal::unix::signal(signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(err) => {
                tracing::warn!(error = %err, "could not install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("ctrl-c received"),
        _ = terminate => tracing::info!("SIGTERM received"),
    }
}

fn mock_mounts() -> Vec<disk_monitor_core::Mount> {
    use disk_monitor_core::{Mount, Usage};
    vec![
        Mount {
            mount_point: "/".into(),
            device: "/dev/mock-root".into(),
            fs_type: "ext4".into(),
            usage: Usage {
                used_bytes: 250 * 1024 * 1024 * 1024,
                free_bytes: 250 * 1024 * 1024 * 1024,
                total_bytes: 500 * 1024 * 1024 * 1024,
            },
            largest_files: Vec::new(),
            largest_files_scanned_at: None,
        },
        Mount {
            mount_point: "/home".into(),
            device: "/dev/mock-home".into(),
            fs_type: "ext4".into(),
            usage: Usage {
                used_bytes: 800 * 1024 * 1024 * 1024,
                free_bytes: 200 * 1024 * 1024 * 1024,
                total_bytes: 1024 * 1024 * 1024 * 1024,
            },
            largest_files: Vec::new(),
            largest_files_scanned_at: None,
        },
    ]
}
