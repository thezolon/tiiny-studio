//! `tiiny-studio-server`: the studio without a window, for Docker and LAN hosting.

use studio_core::{version, Config};

#[tokio::main]
async fn main() {
    // A source checkout keeps ./data next to the binary's working folder, like the Python server.
    let config = Config::from_env(std::path::PathBuf::from("data"));
    #[cfg(unix)]
    run_as_data_owner(&config.data_dir);
    let addr = format!("{}:{}", config.host, config.port);
    let url = format!("http://localhost:{}/", config.port);
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(_) => {
            eprintln!("Port {} is already in use. If Tiiny Studio is already running, it's at {url}", config.port);
            std::process::exit(1);
        }
    };
    println!("Tiiny Studio {} on {addr} (data: {})", version(), config.data_dir.display());
    if config.lan_access && config.password.is_empty() {
        println!("WARNING: reachable from your network without a password (set STUDIO_PASSWORD).");
    }
    if let Err(e) = studio_core::serve(listener, config).await {
        eprintln!("server stopped: {e}");
        std::process::exit(1);
    }
}

/// Docker on Linux runs us as root, so results in the mounted data folder would belong to root and the user
/// couldn't delete them without sudo. Take on the folder's owner instead, and hand back anything an earlier
/// root run left behind. A root-owned folder (a named volume, one Docker created, Docker Desktop's view of a
/// Windows folder) goes to an unprivileged user (UNPRIVILEGED) instead: the server never keeps real root.
/// Rootless Docker/Podman is left alone: its "root" is the host user already.
#[cfg(unix)]
fn run_as_data_owner(data: &std::path::Path) {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    const UNPRIVILEGED: u32 = 10001;
    if unsafe { libc::geteuid() } != 0 || !real_root() {
        return;
    }
    let _ = std::fs::create_dir_all(data);
    let Ok(st) = std::fs::metadata(data) else { refuse(data, "can't read the data folder") };
    let (uid, gid) = if st.uid() == 0 { (UNPRIVILEGED, UNPRIVILEGED) } else { (st.uid(), st.gid()) };
    fn walk(path: &std::path::Path, uid: u32, gid: u32) {
        if let Ok(m) = std::fs::symlink_metadata(path) {
            if m.uid() == 0 {
                if let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) {
                    unsafe { libc::lchown(c.as_ptr(), uid, gid) };
                }
            }
            if m.is_dir() {
                for entry in std::fs::read_dir(path).into_iter().flatten().flatten() {
                    walk(&entry.path(), uid, gid);
                }
            }
        }
    }
    walk(data, uid, gid);
    // some mounts refuse chown (Docker Desktop's Windows folders); they're writable by everyone instead
    let writable = std::fs::metadata(data).is_ok_and(|m| m.uid() == uid || m.permissions().mode() & 0o002 != 0);
    if !writable {
        refuse(data, &format!("user {uid} can't write to it; give it to your user (sudo chown -R $(id -u):$(id -g) <folder>)"));
    }
    // drop root's supplementary groups first (only root may), then the group, then the user
    let (groups, group, user) = unsafe { (libc::setgroups(0, std::ptr::null()), libc::setgid(gid), libc::setuid(uid)) };
    if groups != 0 || group != 0 || user != 0 || unsafe { libc::geteuid() } == 0 {
        let e = std::io::Error::last_os_error();
        refuse(data, &format!("couldn't switch to uid {uid}, gid {gid}: {e}"));
    }
}

/// Root of the whole machine, not of a user namespace (rootless containers map their root to a normal user).
#[cfg(unix)]
fn real_root() -> bool {
    std::fs::read_to_string("/proc/self/uid_map").map_or(true, |m| m.split_whitespace().collect::<Vec<_>>() == ["0", "0", "4294967295"])
}

#[cfg(unix)]
fn refuse(data: &std::path::Path, why: &str) -> ! {
    eprintln!("Not starting as root with the data folder {}: {why}.", data.display());
    std::process::exit(1);
}
