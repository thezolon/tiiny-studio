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
/// root run left behind. (Docker Desktop on Windows shows the folder as root's: no change.)
#[cfg(unix)]
fn run_as_data_owner(data: &std::path::Path) {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    if unsafe { libc::geteuid() } != 0 {
        return;
    }
    let Ok(st) = std::fs::metadata(data) else { return };
    if !st.is_dir() || st.uid() == 0 {
        return;
    }
    let (uid, gid) = (st.uid(), st.gid());
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
    // drop root's supplementary groups first (only root may), then the group, then the user; say so if any fails
    // (a rootless container may refuse setgroups) instead of carrying on as root without a word
    let (groups, group, user) = unsafe { (libc::setgroups(0, std::ptr::null()), libc::setgid(gid), libc::setuid(uid)) };
    if groups != 0 || group != 0 || user != 0 {
        eprintln!(
            "warning: couldn't fully switch to the data folder's owner (uid {uid}, gid {gid}): {}; still running as uid {}",
            std::io::Error::last_os_error(),
            unsafe { libc::geteuid() }
        );
    }
}
