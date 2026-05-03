# disk-monitord HTTP API

Default bind: `127.0.0.1:9126`. All responses are JSON unless noted.

## `GET /healthz`

Liveness probe.

```json
{"status":"ok","uptime_s":42}
```

## `GET /v1/info`

Backend metadata.

```json
{
  "backend_version": "2.0.0-alpha.1",
  "api_version": "v1",
  "host": "wallabot",
  "mount_count": 3
}
```

## `GET /v1/snapshot`

Most recent cached snapshot (refreshed by the sampler at `--sample-interval-ms`).

```json
{
  "timestamp": "2026-05-03T11:45:15.599+00:00",
  "host": "wallabot",
  "mounts": [
    {
      "mount_point": "/",
      "device": "/dev/nvme0n1p2",
      "fs_type": "ext4",
      "usage": {
        "used_bytes": 950107025408,
        "free_bytes": 32713871360,
        "total_bytes": 982820896768
      }
    }
  ]
}
```

`free_bytes` is the space available to non-root users (matches `df` and Python's `shutil.disk_usage`). Pseudo filesystems (tmpfs, sysfs, proc, snap mounts, etc.) are filtered out.

## `GET /v1/mounts`

Lightweight metadata for every mount.

```json
[
  {
    "mount_point": "/",
    "device": "/dev/nvme0n1p2",
    "fs_type": "ext4",
    "total_bytes": 982820896768
  }
]
```

## `GET /v1/mounts/{path}`

Full `Mount` object for the given mount point. The leading `/` of the mount point is stripped in the URL (so `/home` → `/v1/mounts/home`, `/boot/efi` → `/v1/mounts/boot/efi`). Returns `404` if the mount is unknown.

## `POST /v1/rescan`

Trigger an immediate background re-scan of the largest-files cache for **every** mount. The actual walk happens off the request thread; the response returns as soon as the request is queued.

```json
{"queued": true, "target": "*"}
```

Returns `503 Service Unavailable` if the daemon was started with `--no-largest-files`.

## `POST /v1/rescan/{path}`

Trigger an immediate re-scan of just one mount. The leading `/` is stripped in the URL (so `/home` → `POST /v1/rescan/home`, `/` → `POST /v1/rescan` with no extra component).

```json
{"queued": true, "target": "/home"}
```

Returns `404` if `path` does not match any known mount, `503` if the scanner is disabled.

This is what the tray frontend POSTs after a "Move to trash" action so the menu reflects the freed space within one scanner cycle (~14 s for `/`, ~1 ms for fast mounts).

## `GET /v1/stream`

Server-Sent Events. Each event payload is a full `Snapshot` JSON, emitted every `--sample-interval-ms`. Ping comments every 15s keep the connection alive through proxies.

```bash
curl -N http://127.0.0.1:9126/v1/stream
```

## CLI flags

| Flag | Env var | Default | Notes |
|---|---|---|---|
| `--bind` | `DISK_MONITORD_BIND` | `127.0.0.1` | Use `0.0.0.0` to expose on LAN. |
| `--port` | `DISK_MONITORD_PORT` | `9126` | |
| `--sample-interval-ms` | `DISK_MONITORD_SAMPLE_INTERVAL_MS` | `1000` | Floor of 50 ms is enforced. |
| `--log-level` | `RUST_LOG` | `info` | Standard tracing-subscriber EnvFilter syntax. |
| `--mock` | `DISK_MONITORD_MOCK` | `false` | Synthetic data for development. |
| `--largest-top-n` | `DISK_MONITORD_LARGEST_TOP_N` | `20` | How many largest files to keep per mount. |
| `--largest-refresh-secs` | `DISK_MONITORD_LARGEST_REFRESH_SECS` | `300` | Seconds between scheduled re-scans. On-demand `POST /v1/rescan` ignores this. |
| `--largest-initial-delay-secs` | `DISK_MONITORD_LARGEST_INITIAL_DELAY_SECS` | `30` | Delay before the first scan, so daemon startup doesn't fight login I/O. |
| `--no-largest-files` | `DISK_MONITORD_NO_LARGEST_FILES` | `false` | Disable the scanner; rescan endpoints respond `503`. |

## Authentication

The current release is unauthenticated and bound to `127.0.0.1` by default.
