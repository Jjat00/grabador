use grabador_core::{self as core, Config, Monitor, Options, PreviewStream, Recording, State};
use serde::Serialize;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use tauri::ipc::Response;
use tauri::{AppHandle, Manager};

type Res<T> = Result<T, String>;

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Res<T> + Send + 'static) -> Res<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?
}

/// Solo se abren archivos de la carpeta de grabaciones.
fn checked(path: &str) -> Res<&str> {
    if Path::new(path).starts_with(core::get_config().out_dir) {
        Ok(path)
    } else {
        Err("Ruta fuera de la carpeta de grabaciones".into())
    }
}

fn explorer(arg: String) -> Res<()> {
    Command::new("explorer").raw_arg(arg).spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Mantiene la vista previa en vivo mientras no se graba: sigue al monitor
/// elegido (o al del puntero) y se pausa con la ventana minimizada.
fn keep_preview(app: AppHandle) {
    let mut stream: Option<PreviewStream> = None;
    loop {
        let minimized = app.get_webview_window("main").is_some_and(|w| w.is_minimized().unwrap_or(false));
        let wanted = (!minimized && !core::status().is_active()).then(core::target_monitor);
        if stream.as_mut().is_some_and(|s| Some(s.monitor()) != wanted || !s.alive()) {
            stream = None;
        }
        if let (None, Some(monitor)) = (&stream, wanted) {
            stream = PreviewStream::start(monitor).ok();
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

#[derive(Serialize)]
struct AppStatus {
    #[serde(flatten)]
    state: State,
    /// Monitor que se está grabando, o el que se grabaría ahora.
    target: u32,
}

#[tauri::command]
fn status() -> AppStatus {
    let state = core::status();
    let target = state.monitor.filter(|_| state.is_active()).unwrap_or_else(core::target_monitor);
    AppStatus { state, target }
}

#[tauri::command]
fn get_config() -> Config {
    core::get_config()
}

#[tauri::command]
fn set_config(config: Config) -> Config {
    core::save_config(&config);
    config
}

#[tauri::command]
fn monitors() -> Vec<Monitor> {
    core::monitors()
}

#[tauri::command]
async fn devices() -> Res<Vec<String>> {
    blocking(core::audio_devices).await
}

#[tauri::command]
async fn start() -> Res<State> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    blocking(move || core::start(Options::default(), &exe)).await
}

#[tauri::command]
async fn stop() -> Res<State> {
    blocking(core::stop).await
}

#[tauri::command]
async fn screenshot() -> Res<String> {
    blocking(|| core::screenshot(None)).await
}

/// Cuadro actual de la vista previa (JPEG); vacío si todavía no hay.
#[tauri::command]
fn live_preview() -> Response {
    Response::new(core::live_preview().unwrap_or_default())
}

#[tauri::command]
fn recordings(limit: usize) -> Vec<Recording> {
    core::list_recordings(limit)
}

#[tauri::command]
fn open_file(path: String) -> Res<()> {
    explorer(format!("\"{}\"", checked(&path)?))
}

#[tauri::command]
fn reveal(path: String) -> Res<()> {
    explorer(format!("/select,\"{}\"", checked(&path)?))
}

#[tauri::command]
fn open_folder() -> Res<()> {
    let dir = core::get_config().out_dir;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    explorer(format!("\"{dir}\""))
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle().clone();
            std::thread::spawn(move || keep_preview(handle));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            status, get_config, set_config, monitors, devices, start, stop, screenshot, live_preview, recordings,
            open_file, reveal, open_folder
        ])
        .run(tauri::generate_context!())
        .expect("error al iniciar Grabador");
}
