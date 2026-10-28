//! Tiiny Studio desktop: starts the studio server inside the app on 127.0.0.1:<free port> and shows it in
//! a window. The window loads the page over plain HTTP (not a custom protocol) so fetches, pdf.js
//! workers and downloads behave as they do in a browser.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use studio_core::{user_data_dir, Config};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_opener::OpenerExt;

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Same call as Settings → Power off: refused (409) while jobs run; the answer is shown either way.
fn power_off(app: &AppHandle, port: u16) {
    let app = app.clone();
    app.dialog()
        .message("Power off the Tiiny now? It shuts down in a few seconds; you'll need to press its power button to turn it back on.")
        .title("Power off the Tiiny")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom("Power off".into(), "Cancel".into()))
        .show(move |yes| {
            if !yes {
                return;
            }
            tauri::async_runtime::spawn(async move {
                let res = reqwest::Client::new().post(format!("http://127.0.0.1:{port}/api/device/shutdown")).send().await;
                let msg = match res {
                    Ok(r) if r.status().is_success() => {
                        if device_goes_off(port).await {
                            "The Tiiny has powered off.".to_string()
                        } else {
                            "The Tiiny said it would power off but it's still on. Press its power button to turn it off.".to_string()
                        }
                    }
                    Ok(r) => {
                        let v: serde_json::Value = r.json().await.unwrap_or_default();
                        v.pointer("/error/message").and_then(|m| m.as_str()).unwrap_or("The Tiiny didn't accept the request.").to_string()
                    }
                    Err(e) => format!("Couldn't reach the studio: {e}"),
                };
                app.dialog().message(msg).title("Power off the Tiiny").show(|_| {});
            });
        });
}

/// The device answers "ok" even when its shutdown fails (TiinyOS 0.1.36), so check it actually went away:
/// true once it stops answering, false if it's still up after a minute.
async fn device_goes_off(port: u16) -> bool {
    let http = reqwest::Client::new();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while std::time::Instant::now() < until {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        let up = http.get(format!("http://127.0.0.1:{port}/api/models")).send().await.map(|r| r.status().is_success()).unwrap_or(false);
        if !up {
            return true;
        }
    }
    false
}

/// `dir/name`, or `dir/name (2).ext`, `(3)`… if that file already exists.
fn unique_path(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    (2..10_000).map(|n| dir.join(format!("{stem} ({n}){ext}"))).find(|p| !p.exists()).unwrap_or(first)
}

fn main() {
    // Bind before the window exists so the port is known; only this computer can reach it.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("no free local port");
    listener.set_nonblocking(true).expect("listener setup");
    let port = listener.local_addr().expect("listener address").port();

    let mut config = Config::from_env(user_data_dir());
    config.host = "127.0.0.1".into();
    config.port = port;
    config.lan_access = false; // LAN sharing is opt-in (Stage 7)
    config.app = true;
    let data_dir = config.data_dir.clone();

    tauri::Builder::default()
        // a second launch just brings the running window forward
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, None))
        .setup(move |app| {
            tauri::async_runtime::spawn(async move {
                let listener = tokio::net::TcpListener::from_std(listener).expect("listener");
                if let Err(e) = studio_core::serve(listener, config).await {
                    eprintln!("studio server stopped: {e}");
                }
            });
            let url = format!("http://127.0.0.1:{port}/").parse().expect("local url");
            let handle = app.handle().clone();
            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                // links that open a new window (the guide, the credit link) go to the system browser instead
                .on_new_window(move |url, _features| {
                    let _ = handle.opener().open_url(url.as_str(), None::<&str>);
                    tauri::webview::NewWindowResponse::Deny
                })
                // the page's Download/Export/Save buttons are <a download> links (files and blob: data); a webview
                // drops them unless the app says where they go: the Downloads folder, never overwriting
                .on_download(|webview, event| {
                    match event {
                        tauri::webview::DownloadEvent::Requested { url, destination } => {
                            let name = destination
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .filter(|n| !n.is_empty())
                                .or_else(|| url.path_segments().and_then(|mut s| s.next_back().map(String::from)))
                                .unwrap_or_else(|| "download".into());
                            let dir = webview.app_handle().path().download_dir().unwrap_or_else(|_| std::env::temp_dir());
                            *destination = unique_path(&dir, &name);
                        }
                        tauri::webview::DownloadEvent::Finished { path, success, .. } => {
                            let msg = match (success, path) {
                                (true, Some(p)) => format!("Saved to {}", p.display()),
                                (true, None) => "Saved to your Downloads folder".into(),
                                (false, _) => "The download didn't finish".into(),
                            };
                            let kind = if success { "" } else { "err" };
                            let js = format!("typeof toast === 'function' && toast({}, {})", serde_json::json!(msg), serde_json::json!(kind));
                            let _ = webview.eval(&js);
                        }
                        _ => {}
                    }
                    true
                })
                .title("Tiiny Studio")
                .inner_size(1280.0, 860.0)
                .min_inner_size(720.0, 520.0)
                .build()?;

            let open = MenuItem::with_id(app, "open", "Open Tiiny Studio", true, None::<&str>)?;
            let folder = MenuItem::with_id(app, "folder", "Open data folder", true, None::<&str>)?;
            let power = MenuItem::with_id(app, "power", "Power off the Tiiny…", true, None::<&str>)?;
            let at_login = app.autolaunch().is_enabled().unwrap_or(false);
            let login = CheckMenuItem::with_id(app, "login", "Start at login", true, at_login, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let sep = || PredefinedMenuItem::separator(app);
            let menu = Menu::with_items(app, &[&open, &folder, &login, &sep()?, &power, &sep()?, &quit])?;
            let login_item = login.clone();
            let data = data_dir.clone();
            TrayIconBuilder::with_id("tray")
                .icon(app.default_window_icon().cloned().expect("app icon"))
                .tooltip("Tiiny Studio")
                .menu(&menu)
                .on_menu_event(move |app, event| match event.id().as_ref() {
                    "open" => show_main(app),
                    "folder" => {
                        let _ = std::fs::create_dir_all(&data);
                        let _ = app.opener().open_path(data.to_string_lossy(), None::<&str>);
                    }
                    "power" => power_off(app, port),
                    "login" => {
                        // off by default; the menu tick follows what the OS actually has
                        let auto = app.autolaunch();
                        let _ = if auto.is_enabled().unwrap_or(false) { auto.disable() } else { auto.enable() };
                        let _ = login_item.set_checked(auto.is_enabled().unwrap_or(false));
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Tiiny Studio")
        .run(move |app, event| {
            // Before exiting, stop what we started on the Tiiny: it keeps rendering if we just disappear.
            if let tauri::RunEvent::ExitRequested { api, .. } = &event {
                if !QUITTING.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    api.prevent_exit();
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        stop_running_jobs(port).await;
                        app.exit(0);
                    });
                }
            }
        });
}

static QUITTING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Cancel the studio's running jobs through its own API and wait (at most 20 s) until they've ended.
async fn stop_running_jobs(port: u16) {
    let base = format!("http://127.0.0.1:{port}");
    let http = reqwest::Client::new();
    let running = |jobs: &serde_json::Value| -> Vec<String> {
        jobs.as_array()
            .into_iter()
            .flatten()
            .filter(|j| j["status"] == "running")
            .filter_map(|j| j["id"].as_str().map(String::from))
            .collect()
    };
    let list = || async { http.get(format!("{base}/api/jobs")).send().await.ok()?.json::<serde_json::Value>().await.ok() };
    let Some(jobs) = list().await else { return };
    let ids = running(&jobs);
    if ids.is_empty() {
        return;
    }
    for id in &ids {
        let _ = http.post(format!("{base}/api/jobs/{id}/cancel")).body("{}").send().await;
    }
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while std::time::Instant::now() < until {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if list().await.map(|j| running(&j).is_empty()).unwrap_or(true) {
            break;
        }
    }
}
