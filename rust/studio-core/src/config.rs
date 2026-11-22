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
            port: match env("PORT").trim() {
                "" => 7860,
                p => p.parse().unwrap_or_else(|_| {
                    eprintln!("PORT={p} isn't a port number (1-65535); using 7860.");
                    7860
                }),
            },
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

/// Makes the data folder (and its parents as needed). When the studio creates it, it is readable by this user only on
/// Unix (0700), so the chats, pictures and voices inside can't be read by other accounts on a shared computer
/// whatever the umask made the files. A folder that already exists is left as it is (the user may have shared it on
/// purpose), and a refused chmod (exFAT, a network share) is a warning, never a failure; on Windows the profile's own
/// rights apply.
pub fn private_dir(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(unix)]
    let existed = path.is_dir();
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // not in Docker: there the folder is a bind mount owned by the image's user, and the host's user must
        // still be able to open the results (the container has no other accounts anyway)
        if !existed && !crate::settings::in_docker() {
            if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)) {
                crate::store::warn_data(format!(
                    "The data folder could not be made private to this account ({}).",
                    crate::store::io_reason(&e)
                ));
            }
        }
    }
    Ok(())
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

#[cfg(all(test, unix))]
mod tests {
    #[test]
    fn data_folder_is_private_when_created_and_left_alone_when_it_exists() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |d: &std::path::Path| std::fs::metadata(d).unwrap().permissions().mode() & 0o777;
        let dir = std::env::temp_dir().join(format!("studio-private-{}", std::process::id()));
        super::private_dir(&dir.join("data")).unwrap();
        assert_eq!(mode(&dir.join("data")), 0o700);
        // a folder that was already there (maybe shared on purpose) is not re-tightened
        std::fs::set_permissions(dir.join("data"), std::fs::Permissions::from_mode(0o750)).unwrap();
        super::private_dir(&dir.join("data")).unwrap();
        assert_eq!(mode(&dir.join("data")), 0o750);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
