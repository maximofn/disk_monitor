# Disk monitor

Disk usage indicator for the Ubuntu/GNOME panel: live mount-by-mount usage with a per-mount donut and percentage. Daemon (`disk-monitord`) sits on `/proc/mounts` + `statvfs`, frontend (`disk-monitor-tray`) talks to it over HTTP/SSE.

![disk monitor](disk_monitor.gif)

The legacy Python script lives in `legacy/disk_monitor.py` and still works; the Rust rewrite below is the recommended path going forward (RSS ~10× lower, CPU ~100× lower).

> **Sister monitors:** [gpu_monitor](https://github.com/maximofn/gpu_monitor), `cpu_monitor`, `ram_monitor`. Independent repos so you can install only the ones that match your hardware.

## Architecture

Cargo workspace, three crates plus a Swift Package for the macOS frontend:

```
crates/disk-monitor-core   →  shared serde types (Snapshot / Mount / Usage)
crates/disk-monitord       →  HTTP+SSE daemon (binds 127.0.0.1:9126 by default)
crates/disk-monitor-tray   →  Linux system-tray frontend (ksni + tiny-skia + freetype)
front-mac/                 →  macOS menu bar frontend (Swift + AppKit)
```

API: REST + Server-Sent Events, JSON payloads. See [`docs/api.md`](docs/api.md).

## Build

```bash
cargo build --release --workspace
```

## Run (manual, for development)

```bash
./target/release/disk-monitord                        # foreground daemon
./target/release/disk-monitor-tray                    # tray icon (separate terminal)
```

Daemon listens on `127.0.0.1:9126`. Override with `--bind`/`--port` or `DISK_MONITORD_BIND` / `DISK_MONITORD_PORT`.

To verify with curl:

```bash
curl http://127.0.0.1:9126/v1/snapshot | jq
curl -N http://127.0.0.1:9126/v1/stream
```

## Install

Build, then copy binaries + assets + service files into your home tree:

```bash
cargo build --release --workspace

mkdir -p ~/.local/bin ~/.local/share/disk-monitor ~/.config/systemd/user ~/.config/autostart
install -m 0755 target/release/disk-monitord     ~/.local/bin/
install -m 0755 target/release/disk-monitor-tray ~/.local/bin/
install -m 0644 assets/disk.png                  ~/.local/share/disk-monitor/
install -m 0644 packaging/systemd/disk-monitord.service ~/.config/systemd/user/
install -m 0644 packaging/autostart/disk-monitor-tray.desktop ~/.config/autostart/

systemctl --user daemon-reload
systemctl --user enable --now disk-monitord
```

The tray autostarts on next login. To start it now without logging out:

```bash
nohup ~/.local/bin/disk-monitor-tray >/dev/null 2>&1 & disown
```

> The tray is a `.desktop` autostart, **not** a systemd service — it needs the
> graphical session (DBus user bus + panel) to be up before it can plant its
> icon. After rebuilding the tray binary, restart it with `pkill` + relaunch
> (or just log out / log in); `systemctl --user restart disk-monitor-tray`
> won't work because no such unit exists.

## macOS frontend

For consuming the same daemon from a Mac (menu bar app, no Dock icon):

```bash
cd front-mac
./scripts/build-app.sh                       # produces build/Disk Monitor.app
open 'build/Disk Monitor.app' --args --backend-url http://127.0.0.1:9126
```

If the daemon runs on a remote Linux host (the recommended setup — daemon
bindeado a `127.0.0.1`, sin auth), keep an SSH tunnel alive at login:

```bash
# Edit the SSH host in the plist first (default: wallabot)
sed -i '' 's/wallabot/<your-ssh-host>/' scripts/com.maximofn.disk-monitor-tunnel.plist
./scripts/install-tunnel.sh                  # LaunchAgent: ssh -N -L 9126:127.0.0.1:9126
./scripts/install-launchagent.sh             # LaunchAgent: Disk Monitor.app at login
```

Same renderer geometry as the Linux tray (per-mount donut + label + percent
inside) but with Core Graphics + Core Text instead of tiny-skia + freetype.
The Mac tray's mount submenu also exposes a "Rescan largest files" action
that POSTs to `/v1/rescan/{mount}`. See [`front-mac/`](front-mac/) for
details.

### Legacy (Python)

If you still want to run `legacy/disk_monitor.py` instead of the Rust path:

```bash
sudo apt install python3-pip
pip3 install psutil matplotlib
```

## Dev tips

```bash
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all

# Render the panel icon to a PNG and exit (no DBus, no GNOME):
./target/release/disk-monitor-tray --backend-url http://127.0.0.1:9126 --dump-icon /tmp/icon.png
```

`--mock` on the daemon serves a synthetic two-mount snapshot, useful in CI or on a machine where `/proc/mounts` is locked down.

## Home Assistant integration

Surface disk state as native HA sensors with no custom component — just a YAML package on top of `default_config`'s `rest` integration. Polls `/v1/snapshot` every 60 s (disk usage doesn't move second-by-second, and `largest_files` scans run on a background timer in the daemon) and exposes 18 entities: host metadata + 8 sensors per hardcoded mount (`/` and `/media/wallabot/seagate2T`) — device, fs_type, total/used/free in GiB, used %, largest file name + size. The `largest_file` sensor carries the full top-N list as `attributes.largest_files`.

```bash
# On the raspberry running Home Assistant:
cd home-assistant/tunnel
./install.sh                                 # generates dedicated SSH key, installs systemd user unit
# (paste the printed pubkey line into the disk host's ~/.ssh/authorized_keys)

# Copy the package and reload HA:
cp ../packages/disk_monitor.yaml /config/packages/
docker restart homeassistant
```

The dedicated key is restricted with `restrict,port-forwarding,permitopen="127.0.0.1:9126"`. Mount lookup is done with `selectattr` on `mount_point` (not by index), so the daemon's mount order doesn't matter. To add a new mount: copy a block and swap the slug; instructions in [`home-assistant/README.md`](home-assistant/README.md).

## Support

If this is useful, give the repo a ★. If you want to buy me a coffee:

[![BuyMeACoffee](https://img.shields.io/badge/Buy_Me_A_Coffee-support_my_work-FFDD00?style=for-the-badge&logo=buy-me-a-coffee&logoColor=white&labelColor=101010)](https://www.buymeacoffee.com/maximofn)
