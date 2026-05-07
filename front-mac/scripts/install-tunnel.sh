#!/usr/bin/env bash
# Install / reinstall the LaunchAgent that keeps an `ssh -N -L` tunnel alive
# from this Mac to the disk-monitord port on the Linux host.
# Usage:
#   ./scripts/install-tunnel.sh           # install + load
#   ./scripts/install-tunnel.sh uninstall # unload + remove
#
# Edit com.maximofn.disk-monitor-tunnel.plist first to point `wallabot` at your
# own SSH host (or `sed -i '' 's/wallabot/myhost/' ...`).
set -euo pipefail

LABEL="com.maximofn.disk-monitor-tunnel"
SRC="$(cd "$(dirname "$0")" && pwd)/${LABEL}.plist"
DST="$HOME/Library/LaunchAgents/${LABEL}.plist"

uid="$(id -u)"
domain="gui/${uid}"
target="${domain}/${LABEL}"

cmd="${1:-install}"

case "$cmd" in
    install)
        if launchctl print "$target" >/dev/null 2>&1; then
            echo "==> bootout existing $LABEL"
            launchctl bootout "$target" || true
        fi

        echo "==> install $DST"
        mkdir -p "$HOME/Library/LaunchAgents" "$HOME/Library/Logs"
        cp "$SRC" "$DST"

        echo "==> bootstrap $target"
        launchctl bootstrap "$domain" "$DST"
        launchctl enable "$target"
        launchctl kickstart -k "$target"

        echo
        echo "Tunnel will autostart on login."
        echo "Logs: ~/Library/Logs/disk-monitor-tunnel.{out,err}.log"
        ;;
    uninstall)
        if launchctl print "$target" >/dev/null 2>&1; then
            echo "==> bootout $target"
            launchctl bootout "$target" || true
        fi
        if [[ -f "$DST" ]]; then
            echo "==> remove $DST"
            rm -f "$DST"
        fi
        echo "Uninstalled."
        ;;
    *)
        echo "usage: $0 [install|uninstall]" >&2
        exit 2
        ;;
esac
