//! Tiiny Studio backend. Port of the Python server (server.py, media.py, chat.py, extract.py), which stays
//! the behaviour reference until the Rust version replaces it: same routes, same JSON, same data folder.

pub mod chat;
pub mod config;
pub mod device;
pub mod extract;
pub mod jobs;
mod library;
pub mod media;
mod routes;
pub mod safety;
pub mod settings;
pub mod store;

use std::sync::{Arc, Mutex};
use std::time::Instant;

pub use config::{user_data_dir, Config};
pub use settings::Settings;

pub const VERSION: &str = include_str!("../../../VERSION");

pub fn version() -> &'static str {
    VERSION.trim()
}

/// (when read, device url, direct access to the model ports) of device::direct_access
type DirectCache = (Instant, String, Option<bool>);
/// (when read, device url, (free GB, total GB)) of the Tiiny's storage
type DiskCache = (Instant, String, Option<(f64, f64)>);
/// entry id -> (text, vector)
type SearchCache = std::collections::HashMap<String, (String, Vec<f64>)>;

/// Shared state handed to every route.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub settings: Arc<Settings>,
    pub http: reqwest::Client,
    /// The device runs one request at a time; every job (image, audio, 3D) takes this.
    pub device_lock: Arc<tokio::sync::Mutex<()>>,
    pub(crate) direct_cache: Arc<Mutex<Option<DirectCache>>>,
    pub(crate) stats_cache: Arc<Mutex<Option<(Instant, serde_json::Value)>>>,
    /// (when read, device url, (free GB, total GB)) of the Tiiny's storage; see device::disk_space
    pub(crate) disk_cache: Arc<Mutex<Option<DiskCache>>>,
    pub jobs: Arc<jobs::Jobs>,
    pub store: Arc<store::Store>,
    /// entry id -> (text, vector) for smart search; memory only, dropped on delete/wipe
    pub(crate) search_cache: Arc<Mutex<SearchCache>>,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        let _ = std::fs::create_dir_all(&config.data_dir);
        let settings = Settings::load(
            &config.data_dir,
            config.lan_access,
            !config.password.is_empty(),
            config.app,
        );
        let config_dir = config.data_dir.clone();
        AppState {
            settings: Arc::new(settings),
            config: Arc::new(config),
            http: reqwest::Client::new(),
            device_lock: Arc::new(tokio::sync::Mutex::new(())),
            direct_cache: Arc::default(),
            stats_cache: Arc::default(),
            disk_cache: Arc::default(),
            jobs: Arc::default(),
            store: Arc::new(store::Store::new(&config_dir)),
            search_cache: Arc::default(),
        }
    }

    /// Any job queued or running.
    pub fn jobs_active(&self) -> bool {
        self.jobs.any_active()
    }

    /// A job running on this model right now (the catalog's busy flag).
    pub fn jobs_running_on(&self, model: &str) -> bool {
        self.jobs.running_on(model)
    }
}

pub fn build_router(config: Config) -> axum::Router {
    routes::router(AppState::new(config))
}

/// Serve on an already-bound listener (the desktop app binds 127.0.0.1:0 and reads the port back).
/// Also starts the device monitor (auto-switch between saved addresses), every 20 s like server.py.
pub async fn serve(listener: tokio::net::TcpListener, config: Config) -> std::io::Result<()> {
    std::fs::create_dir_all(&config.data_dir)?;
    let state = AppState::new(config);
    let monitor = state.clone();
    tokio::spawn(async move {
        loop {
            monitor.settings.check(&monitor.http).await;
            tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        }
    });
    // the client address decides a few things (e.g. only this computer may open the results folder)
    let jobs = state.clone();
    axum::serve(
        listener,
        routes::router(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        shutdown_signal().await;
        // tell the Tiiny to stop what we started: it keeps rendering if we just disappear
        jobs.jobs
            .cancel_running(&jobs, std::time::Duration::from_secs(20))
            .await;
    })
    .await
}

/// Ctrl+C, or SIGTERM (`docker stop`) on Unix.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
}
