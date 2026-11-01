//! Settings from the environment, the same variables the Python server reads (server.py top of file).

use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Config {
    pub port: u16,
    /// Native runs stay on this computer unless HOST=0.0.0.0.
    pub host: String,
    /// Optional: ask browsers for a password (any username).
    pub password: String,
    /// Docker publishes on STUDIO_BIND; a native run listens on HOST.
    pub lan_access: bool,
    pub data_dir: PathBuf,
    /// Running inside the desktop app (it only serves its own window; there's no .env to edit).
    pub app: bool,
}

impl Config {
    /// `default_data` is where data goes when DATA_DIR isn't set (the app picks the per-user folder).
    pub fn from_env(default_data: PathBuf) -> Self {
        let env = |k: &str| std::env::var(k).unwrap_or_default();
        let host = Some(env("HOST"))
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| "127.0.0.1".into());
        let bind = Some(env("STUDIO_BIND"))
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| host.clone());
        Config {
            port: env("PORT").parse().unwrap_or(7860),
            lan_access: !matches!(bind.as_str(), "127.0.0.1" | "localhost" | "::1"),
            host,
            password: env("STUDIO_PASSWORD"),
            data_dir: Some(env("DATA_DIR"))
                .filter(|d| !d.is_empty())
                .map(PathBuf::from)
                .unwrap_or(default_data),
            app: false,
        }
    }
}

/// Where the standalone app keeps results and settings: the usual per-user place on each OS.
pub fn user_data_dir() -> PathBuf {
    let home = || {
        PathBuf::from(
            std::env::var("HOME")
                .or_else(|_| std::env::var("USERPROFILE"))
                .unwrap_or_default(),
        )
    };
    if cfg!(windows) {
        std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home().join("AppData").join("Roaming"))
            .join("Tiiny Studio")
    } else if cfg!(target_os = "macos") {
        home()
            .join("Library")
            .join("Application Support")
            .join("Tiiny Studio")
    } else {
        std::env::var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home().join(".local").join("share"))
            .join("tiiny-studio")
    }
}
