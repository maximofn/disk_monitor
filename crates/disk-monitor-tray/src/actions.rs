use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Show a yes/no dialog using `zenity`. Returns `true` if the user clicked
/// "Yes". Returns `false` if zenity is not installed or the user cancelled.
/// We deliberately fall back to "no" on errors — losing data accidentally is
/// far worse than silently refusing to delete.
pub fn confirm(title: &str, message: &str) -> bool {
    let status = Command::new("zenity")
        .args([
            "--question",
            "--title",
            title,
            "--text",
            message,
            // Disk Monitor is the parent context, not nautilus / GNOME.
            "--ok-label=Move to trash",
            "--cancel-label=Cancel",
            "--width=480",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(s) => s.success(),
        Err(err) => {
            tracing::warn!(error = %err, "zenity failed; refusing to delete (install zenity for confirmations)");
            false
        }
    }
}

/// Move a single file to the freedesktop.org trash (`~/.local/share/Trash/`).
/// This is the same destination Nautilus uses, so the file is recoverable
/// from the GNOME Files trash UI.
pub fn move_to_trash(path: &Path) -> Result<(), trash::Error> {
    trash::delete(path)
}

/// Open the parent directory of `path` in a file manager, with the file
/// highlighted if the file manager supports `--select`. Tries
/// `nautilus --select` first (GNOME default), then `dolphin --select` (KDE),
/// then falls back to `xdg-open` on the parent directory.
pub fn open_in_file_manager(path: &Path) {
    if try_select(path, "nautilus") {
        return;
    }
    if try_select(path, "dolphin") {
        return;
    }
    let parent = path.parent().unwrap_or(Path::new("/"));
    if let Err(err) = Command::new("xdg-open")
        .arg(parent)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        tracing::warn!(error = %err, parent = %parent.display(), "xdg-open failed");
    }
}

fn try_select(path: &Path, program: &str) -> bool {
    if which(program).is_none() {
        return false;
    }
    match Command::new(program)
        .arg("--select")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(_) => true,
        Err(err) => {
            tracing::warn!(error = %err, program, "spawn failed");
            false
        }
    }
}

fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(program);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// POST `/v1/rescan/{mount}` to ask the daemon to re-scan one mount. Runs
/// synchronously (blocking reqwest) and is meant to be called from a worker
/// thread spawned out of the ksni menu callback.
pub fn trigger_rescan(backend_url: &str, mount_point: &str) {
    let path = mount_point.trim_start_matches('/');
    let url = format!("{}/v1/rescan/{}", backend_url.trim_end_matches('/'), path);
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
    {
        Ok(c) => c,
        Err(err) => {
            tracing::warn!(error = %err, "could not build rescan HTTP client");
            return;
        }
    };
    match client.post(&url).send() {
        Ok(resp) if resp.status().is_success() => {
            tracing::info!(%url, "rescan triggered");
        }
        Ok(resp) => {
            tracing::warn!(status = %resp.status(), %url, "rescan rejected");
        }
        Err(err) => {
            tracing::warn!(error = %err, %url, "rescan POST failed");
        }
    }
}
