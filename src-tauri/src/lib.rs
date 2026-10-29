use grabador_core::{self as core, Config, Options, Recording, State};
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;
use tauri::ipc::Response;

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

#[tauri::command]
fn status() -> State {
    core::status()
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

/// Cuadro actual de la grabación en curso (JPEG); vacío si no se está grabando.
#[tauri::command]
fn live_preview() -> Response {
    Response::new(core::live_preview().unwrap_or_default())
}

/// Imagen actual de un monitor (JPEG) para la vista previa en reposo.
#[tauri::command]
async fn snapshot(monitor: Option<u32>) -> Res<Response> {
    blocking(move || core::snapshot(monitor, 480)).await.map(Response::new)
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
        .invoke_handler(tauri::generate_handler![
            status, get_config, set_config, devices, start, stop, screenshot, live_preview, snapshot, recordings,
            open_file, reveal, open_folder
        ])
        .run(tauri::generate_context!())
        .expect("error al iniciar Grabador");
}
