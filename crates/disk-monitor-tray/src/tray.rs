use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use disk_monitor_core::{FileEntry, Mount, Snapshot};
use ksni::menu::{StandardItem, SubMenu};
use ksni::{MenuItem, ToolTip, Tray};

use crate::actions;
use crate::icon::IconRenderer;

const REPO_URL: &str = "https://github.com/maximofn/disk_monitor";
const COFFEE_URL: &str = "https://www.buymeacoffee.com/maximofn";
const ICON_BASENAME: &str = "disk-monitor-tray";

#[derive(Debug, Clone)]
pub enum State {
    Connecting,
    Connected(Snapshot),
    Disconnected(String),
}

pub struct DiskTray {
    renderer: IconRenderer,
    backend_url: String,
    state: State,
    icon_dir: PathBuf,
    /// Counter that increments on every redraw so the panel sees a new
    /// `IconName` and reloads the file from disk (matches what AppIndicator's
    /// `set_icon_full` does internally — GNOME-shell otherwise caches by name).
    generation: u64,
    current_icon_name: String,
}

impl DiskTray {
    pub fn new(renderer: IconRenderer, backend_url: String, icon_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&icon_dir)
            .with_context(|| format!("creating icon dir {}", icon_dir.display()))?;
        // Wipe any stale icons left by a previous run so the cache stays bounded.
        if let Ok(entries) = std::fs::read_dir(&icon_dir) {
            for entry in entries.flatten() {
                if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(ICON_BASENAME)
                {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        let mut tray = Self {
            renderer,
            backend_url,
            state: State::Connecting,
            icon_dir,
            generation: 0,
            current_icon_name: String::new(),
        };
        tray.refresh_icon_file();
        Ok(tray)
    }

    pub fn set_state(&mut self, state: State) {
        self.state = state;
        self.refresh_icon_file();
    }

    fn refresh_icon_file(&mut self) {
        let png = match self
            .renderer
            .render_png(self.current_mounts(), self.connected())
        {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::warn!(error = %err, "failed to render icon PNG");
                return;
            }
        };
        self.generation = self.generation.wrapping_add(1);
        let new_name = format!("{ICON_BASENAME}-{}", self.generation);
        let new_path = self.icon_dir.join(format!("{new_name}.png"));
        if let Err(err) = std::fs::write(&new_path, &png) {
            tracing::warn!(error = %err, path = %new_path.display(), "failed to write icon PNG");
            return;
        }

        // Drop the previous frame so the cache directory does not grow.
        if !self.current_icon_name.is_empty() {
            let old = self
                .icon_dir
                .join(format!("{}.png", self.current_icon_name));
            let _ = std::fs::remove_file(old);
        }
        self.current_icon_name = new_name;
    }

    fn current_mounts(&self) -> &[Mount] {
        match &self.state {
            State::Connected(snap) => snap.mounts.as_slice(),
            _ => &[],
        }
    }

    fn connected(&self) -> bool {
        matches!(self.state, State::Connected(_))
    }
}

impl Tray for DiskTray {
    fn id(&self) -> String {
        "disk-monitor".to_string()
    }

    fn title(&self) -> String {
        "Disk Monitor".to_string()
    }

    fn icon_name(&self) -> String {
        self.current_icon_name.clone()
    }

    fn icon_theme_path(&self) -> String {
        self.icon_dir.to_string_lossy().into_owned()
    }

    fn tool_tip(&self) -> ToolTip {
        let title = "Disk Monitor".to_string();
        let description = match &self.state {
            State::Connecting => format!("Connecting to {}", self.backend_url),
            State::Connected(snap) => {
                let header = format!("{} mount(s)", snap.mounts.len());
                let body: Vec<String> = snap
                    .mounts
                    .iter()
                    .map(|m| {
                        format!(
                            "{} ({}) — {}/{} ({:.0}%)",
                            m.mount_point,
                            m.fs_type,
                            format_bytes(m.usage.used_bytes),
                            format_bytes(m.usage.total_bytes),
                            m.usage.used_percent(),
                        )
                    })
                    .collect();
                format!("{}\n{}", header, body.join("\n"))
            }
            State::Disconnected(err) => format!("Backend offline: {err}"),
        };
        ToolTip {
            icon_name: String::new(),
            icon_pixmap: Vec::new(),
            title,
            description,
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let mut items: Vec<MenuItem<Self>> = Vec::new();

        match &self.state {
            State::Connecting => {
                items.push(disabled_item(format!(
                    "Connecting to {}…",
                    self.backend_url
                )));
                items.push(MenuItem::Separator);
            }
            State::Disconnected(err) => {
                items.push(disabled_item(format!("Backend offline: {err}")));
                items.push(disabled_item(format!("Backend: {}", self.backend_url)));
                items.push(MenuItem::Separator);
            }
            State::Connected(snap) => {
                for mount in &snap.mounts {
                    items.push(MenuItem::SubMenu(mount_submenu(mount, &self.backend_url)));
                }
                items.push(MenuItem::Separator);
                items.push(disabled_item(format!("Backend: {}", self.backend_url)));
                items.push(disabled_item(format!(
                    "Updated: {}",
                    short_time(&snap.timestamp)
                )));
                items.push(MenuItem::Separator);
            }
        }

        items.push(MenuItem::Standard(StandardItem {
            label: "Repository".into(),
            activate: Box::new(|_| open_url(REPO_URL)),
            ..Default::default()
        }));
        items.push(MenuItem::Standard(StandardItem {
            label: "Buy me a coffee".into(),
            activate: Box::new(|_| open_url(COFFEE_URL)),
            ..Default::default()
        }));
        items.push(MenuItem::Separator);
        items.push(MenuItem::Standard(StandardItem {
            label: "Quit".into(),
            activate: Box::new(|_| std::process::exit(0)),
            ..Default::default()
        }));

        items
    }
}

fn mount_submenu(mount: &Mount, backend_url: &str) -> SubMenu<DiskTray> {
    let header = format!("{} ({})", mount.mount_point, mount.fs_type);
    let mut entries: Vec<MenuItem<DiskTray>> = Vec::new();

    entries.push(disabled_item(format!("Device: {}", mount.device)));
    entries.push(disabled_item(format!(
        "Used: {} ({:.0}%)",
        format_bytes(mount.usage.used_bytes),
        mount.usage.used_percent()
    )));
    entries.push(disabled_item(format!(
        "Free: {}",
        format_bytes(mount.usage.free_bytes)
    )));
    entries.push(disabled_item(format!(
        "Total: {}",
        format_bytes(mount.usage.total_bytes)
    )));

    entries.push(MenuItem::Separator);
    entries.push(MenuItem::SubMenu(largest_files_submenu(mount, backend_url)));

    SubMenu {
        label: header,
        submenu: entries,
        ..Default::default()
    }
}

fn largest_files_submenu(mount: &Mount, backend_url: &str) -> SubMenu<DiskTray> {
    let label = if let Some(scanned) = mount.largest_files_scanned_at.as_deref() {
        format!(
            "Largest files (top {}, scanned {})",
            mount.largest_files.len(),
            short_time(scanned)
        )
    } else {
        "Largest files (scanning…)".to_string()
    };

    let mut entries: Vec<MenuItem<DiskTray>> = Vec::new();
    if mount.largest_files.is_empty() {
        if mount.largest_files_scanned_at.is_some() {
            entries.push(disabled_item("(no readable files found)".into()));
        } else {
            entries.push(disabled_item(
                "Initial scan in progress; refresh after a few minutes…".into(),
            ));
        }
    } else {
        // Flat layout: 3 entries per file (path label + Open + Trash) with a
        // Separator between files. GNOME's ubuntu-appindicators extension
        // does not reliably render menus deeper than 2 levels — nesting each
        // file as its own SubMenu hides the actions silently. Keeping it flat
        // means longer menu but everything is visible and clickable.
        for (i, f) in mount.largest_files.iter().enumerate() {
            if i > 0 {
                entries.push(MenuItem::Separator);
            }
            entries.extend(file_entries(f, &mount.mount_point, backend_url));
        }
        entries.push(MenuItem::Separator);
        let url = backend_url.to_string();
        let mp = mount.mount_point.clone();
        entries.push(MenuItem::Standard(StandardItem {
            label: "Rescan now".into(),
            activate: Box::new(move |_| {
                let url = url.clone();
                let mp = mp.clone();
                std::thread::spawn(move || actions::trigger_rescan(&url, &mp));
            }),
            ..Default::default()
        }));
    }

    SubMenu {
        label,
        submenu: entries,
        ..Default::default()
    }
}

fn file_entries(file: &FileEntry, mount_point: &str, backend_url: &str) -> Vec<MenuItem<DiskTray>> {
    let header = format!("{:>9}  {}", format_bytes(file.size_bytes), file.path);

    let path_for_open = file.path.clone();
    let path_for_trash = file.path.clone();
    let size_human = format_bytes(file.size_bytes);
    let url = backend_url.to_string();
    let mp = mount_point.to_string();

    vec![
        disabled_item(header),
        MenuItem::Standard(StandardItem {
            label: "      ↳ Open in file manager".into(),
            activate: Box::new(move |_| {
                let p = path_for_open.clone();
                tracing::info!(path = %p, "menu: open-in-file-manager clicked");
                std::thread::spawn(move || actions::open_in_file_manager(Path::new(&p)));
            }),
            ..Default::default()
        }),
        MenuItem::Standard(StandardItem {
            label: "      ↳ Move to trash…".into(),
            activate: Box::new(move |_| {
                let p = path_for_trash.clone();
                let size = size_human.clone();
                let url = url.clone();
                let mp = mp.clone();
                tracing::info!(path = %p, "menu: move-to-trash clicked");
                std::thread::spawn(move || handle_trash(p, size, url, mp));
            }),
            ..Default::default()
        }),
    ]
}

fn handle_trash(path: String, size_human: String, backend_url: String, mount_point: String) {
    let prompt = format!(
        "Move this file to the trash?\n\n<b>{}</b>\n{}\n\nIt will go to your home trash and can be restored from GNOME Files.",
        size_human, path
    );
    if !actions::confirm("Disk Monitor — confirm deletion", &prompt) {
        tracing::info!(path = %path, "trash cancelled by user");
        return;
    }
    match actions::move_to_trash(Path::new(&path)) {
        Ok(_) => {
            tracing::info!(path = %path, "moved to trash");
            actions::trigger_rescan(&backend_url, &mount_point);
        }
        Err(err) => {
            tracing::warn!(path = %path, error = %err, "could not move to trash");
            // Surface the failure to the user — silent failure on a destructive
            // action is the worst of both worlds (user thinks the file is gone
            // but it isn't).
            let msg = format!(
                "Could not move file to trash:\n\n{}\n\nReason: {}",
                path, err
            );
            let _ = std::process::Command::new("zenity")
                .args(["--error", "--title=Disk Monitor", "--text", &msg, "--width=480"])
                .status();
        }
    }
}

fn disabled_item(label: String) -> MenuItem<DiskTray> {
    MenuItem::Standard(StandardItem {
        label,
        enabled: false,
        ..Default::default()
    })
}

fn open_url(url: &str) {
    if let Err(err) = open::that(url) {
        tracing::warn!(%url, error = %err, "could not open url");
    }
}

fn format_bytes(bytes: u64) -> String {
    const TIB: f64 = 1024.0 * 1024.0 * 1024.0 * 1024.0;
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= TIB {
        format!("{:.2} TiB", b / TIB)
    } else if b >= GIB {
        format!("{:.2} GiB", b / GIB)
    } else if b >= MIB {
        format!("{:.0} MiB", b / MIB)
    } else {
        format!("{} B", bytes)
    }
}

fn short_time(rfc3339: &str) -> &str {
    rfc3339
        .split('T')
        .nth(1)
        .and_then(|s| s.split('.').next())
        .unwrap_or(rfc3339)
}
