use clap::Parser;
use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub const VERSION: &str = "0.6.0";

#[derive(Parser, Debug)]
#[command(name = "ldicons", version = VERSION, about = "Resource-friendly Wayland desktop icons")]
pub struct Args {
    #[arg(short = 'c', long, default_value_os_t = default_config())]
    pub config: PathBuf,
    #[arg(short = 'p', long, default_value_os_t = default_positions())]
    pub positions: PathBuf,
    #[arg(short = 'l', long)]
    pub log: Option<PathBuf>,
    #[arg(long)]
    pub verbose: bool,
    #[arg(long, help = "Scan the desktop and exit without connecting to Wayland")]
    pub check: bool,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub desktop: PathBuf,
    pub show_hidden: bool,
    pub grid_x: i32,
    pub grid_y: i32,
    pub offset_x: i32,
    pub offset_y: i32,
    pub nsd_enabled: bool,
    pub nsd_socket: PathBuf,
    pub file_manager: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            desktop: xdg_desktop_dir(),
            show_hidden: true,
            grid_x: 140,
            grid_y: 180,
            offset_x: 50,
            offset_y: 50,
            nsd_enabled: true,
            nsd_socket: PathBuf::from("/tmp/nsd.sock"),
            file_manager: "xdg-open".into(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> io::Result<Self> {
        let mut cfg = Self::default();
        let text = fs::read_to_string(path)?;
        let mut section = String::new();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
                section = name.to_ascii_lowercase();
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim().trim_matches('"');
            match (section.as_str(), key.as_str()) {
                ("layout", "grid_spacing_x") => cfg.grid_x = value.parse().unwrap_or(cfg.grid_x),
                ("layout", "grid_spacing_y") => cfg.grid_y = value.parse().unwrap_or(cfg.grid_y),
                ("layout", "start_offset_x") => {
                    cfg.offset_x = value.parse().unwrap_or(cfg.offset_x)
                }
                ("layout", "start_offset_y") => {
                    cfg.offset_y = value.parse().unwrap_or(cfg.offset_y)
                }
                ("behavior", "show_hidden_files") => {
                    cfg.show_hidden = value.eq_ignore_ascii_case("true")
                }
                ("daemon", "enabled") => cfg.nsd_enabled = value.eq_ignore_ascii_case("true"),
                ("daemon", "socket_path") => cfg.nsd_socket = expand_path(value),
                ("daemon", "file_manager") => cfg.file_manager = value.to_owned(),
                _ => {}
            }
        }
        cfg.desktop = xdg_desktop_dir();
        Ok(cfg)
    }
}

pub fn home_path(suffix: &str) -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(suffix)
}

fn default_config() -> PathBuf {
    home_path(".config/ldicons/ldicons.conf")
}

fn default_positions() -> PathBuf {
    home_path(".config/ldicons/icon_positions.json")
}

pub fn expand_path(value: &str) -> PathBuf {
    if let Some(rest) = value.strip_prefix("~/") {
        home_path(rest)
    } else {
        PathBuf::from(value)
    }
}

fn xdg_desktop_dir() -> PathBuf {
    if let Some(desktop) = std::env::var_os("XDG_DESKTOP_DIR") {
        return PathBuf::from(desktop);
    }

    let user_dirs = home_path(".config/user-dirs.dirs");
    let Ok(contents) = fs::read_to_string(user_dirs) else {
        return home_path("Desktop");
    };
    let Some(value) = contents.lines().find_map(|line| {
        let line = line.trim();
        line.strip_prefix("XDG_DESKTOP_DIR=").map(|value| {
            value
                .trim_matches('"')
                .replace("$HOME", &home_path("").to_string_lossy())
        })
    }) else {
        return home_path("Desktop");
    };
    expand_path(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_home_paths() {
        let path = expand_path("~/Desktop");
        assert!(path.ends_with("Desktop"));
        assert!(!path.to_string_lossy().starts_with("~/"));
    }

    #[test]
    fn xdg_desktop_dir_falls_back_to_home_desktop() {
        assert!(xdg_desktop_dir().is_absolute());
    }
}
