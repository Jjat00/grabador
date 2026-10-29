//! Motor de grabación compartido por la app, la CLI y el servidor MCP.
//!
//! Graba con ffmpeg + ddagrab (Desktop Duplication API) a resolución nativa y
//! codifica por GPU cuando hay. El estado vive en `%LOCALAPPDATA%\tech.jaimeaza.grabador`,
//! así cualquier interfaz ve (y puede detener) una grabación iniciada por otra.

mod overlay;

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub type Result<T> = std::result::Result<T, String>;

const NO_WINDOW: u32 = 0x0800_0000;
const DETACHED: u32 = 0x0000_0008 | 0x0000_0200; // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP

/// Argumento con el que los binarios se relanzan como proceso de grabación.
pub const WORKER_ARG: &str = "__worker";

// encoder, argumentos ({q} = calidad), formato de píxel. Orden = preferencia.
const ENCODERS: &[(&str, &[&str], &str)] = &[
    ("h264_nvenc", &["-preset", "p5", "-rc", "vbr", "-cq", "{q}", "-b:v", "0"], "yuv420p"),
    ("h264_qsv", &["-preset", "faster", "-global_quality", "{q}"], "nv12"),
    ("h264_amf", &["-quality", "quality", "-rc", "cqp", "-qp_i", "{q}", "-qp_p", "{q}"], "yuv420p"),
    ("libx264", &["-preset", "veryfast", "-crf", "{q}"], "yuv420p"),
];

// ---------- rutas ----------

fn env_path(var: &str) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

pub fn app_dir() -> PathBuf {
    env_path("LOCALAPPDATA").join("tech.jaimeaza.grabador")
}
fn state_file() -> PathBuf {
    app_dir().join("state.json")
}
fn config_file() -> PathBuf {
    app_dir().join("config.json")
}
fn stop_flag() -> PathBuf {
    app_dir().join("stop.flag")
}
fn log_file() -> PathBuf {
    app_dir().join("ffmpeg.log")
}
fn preview_file() -> PathBuf {
    app_dir().join("preview.jpg")
}

// ---------- tipos ----------

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    pub fps: u32,
    /// "maxima" (mejor imagen) o "alta" (archivo más ligero)
    pub quality: String,
    pub monitor: u32,
    /// Dispositivos de audio dshow (sistema y/o micrófono)
    pub audio: Vec<String>,
    pub out_dir: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoder: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            fps: 60,
            quality: "maxima".into(),
            monitor: 0,
            audio: vec![],
            out_dir: env_path("USERPROFILE").join("Videos").join("Grabador").to_string_lossy().into(),
            encoder: None,
        }
    }
}

/// Opciones de una grabación; lo que falte se toma de la configuración.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Options {
    pub fps: Option<u32>,
    pub quality: Option<String>,
    pub monitor: Option<u32>,
    pub audio: Option<Vec<String>>,
    pub out_dir: Option<String>,
    pub duration: Option<f64>,
    pub name: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Job {
    fps: u32,
    quality: String,
    monitor: u32,
    audio: Vec<String>,
    out_dir: String,
    duration: Option<f64>,
    name: String,
    encoder: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct State {
    /// idle | starting | recording | finishing
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fps: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_size_mb: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_duration_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

impl State {
    fn idle(last_file: Option<String>, last_error: Option<String>) -> Self {
        Self { status: "idle".into(), last_file, last_error, ..Default::default() }
    }
    pub fn is_active(&self) -> bool {
        matches!(self.status.as_str(), "starting" | "recording" | "finishing")
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct Recording {
    pub file: String,
    pub size_mb: f64,
    pub modified: String,
}

// ---------- utilidades ----------

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

fn size_mb(path: &Path) -> f64 {
    fs::metadata(path).map(|m| round1(m.len() as f64 / 1_048_576.0)).unwrap_or(0.0)
}

fn read_json<T: DeserializeOwned + Default>(path: &Path) -> T {
    fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn write_json<T: Serialize>(path: &Path, data: &T) {
    let _ = fs::create_dir_all(app_dir());
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, serde_json::to_vec_pretty(data).unwrap_or_default()).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

fn hidden(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.creation_flags(NO_WINDOW);
    cmd
}

fn pid_alive(pid: u32) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        let mut code = 0u32;
        let ok = GetExitCodeProcess(h, &mut code).is_ok();
        let _ = CloseHandle(h);
        ok && code == 259 // STILL_ACTIVE
    }
}

/// Evita que el worker herede stdin/stdout: si heredara el pipe de la terminal o
/// del cliente MCP, éstos esperarían a que terminara la grabación.
fn detach_std_handles() {
    use windows::Win32::Foundation::{SetHandleInformation, HANDLE_FLAGS, HANDLE_FLAG_INHERIT};
    use windows::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
    unsafe {
        for id in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            if let Ok(h) = GetStdHandle(id) {
                if !h.is_invalid() {
                    let _ = SetHandleInformation(h, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0));
                }
            }
        }
    }
}

pub fn ffmpeg() -> Result<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    dirs.push(env_path("LOCALAPPDATA").join("Microsoft").join("WinGet").join("Links"));
    dirs.into_iter()
        .map(|d| d.join("ffmpeg.exe"))
        .find(|p| p.is_file())
        .ok_or_else(|| "No se encontró ffmpeg. Instálalo con: winget install ffmpeg".into())
}

// ---------- configuración ----------

pub fn get_config() -> Config {
    read_json(&config_file())
}

pub fn save_config(cfg: &Config) {
    write_json(&config_file(), cfg)
}

/// Prueba los encoders por hardware y cachea el primero que funcione.
pub fn detect_encoder() -> Result<String> {
    let mut cfg = get_config();
    if let Some(enc) = &cfg.encoder {
        return Ok(enc.clone());
    }
    let ff = ffmpeg()?;
    let found = ENCODERS
        .iter()
        .find(|(name, _, pix)| {
            hidden(&ff)
                .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "color=c=black:s=1280x720:r=30"])
                .args(["-frames:v", "10", "-pix_fmt", pix, "-c:v", name, "-f", "null", "-"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        })
        .map_or("libx264", |(name, _, _)| name);
    cfg.encoder = Some(found.into());
    save_config(&cfg);
    Ok(found.into())
}

pub fn audio_devices() -> Result<Vec<String>> {
    let out = hidden(&ffmpeg()?)
        .args(["-hide_banner", "-list_devices", "true", "-f", "dshow", "-i", "dummy"])
        .output()
        .map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&out.stderr)
        .lines()
        .filter(|l| l.ends_with("(audio)"))
        .filter_map(|l| l.split('"').nth(1).map(String::from))
        .collect())
}

// ---------- ffmpeg ----------

fn capture_input(monitor: u32, fps: u32) -> Vec<String> {
    let graph = format!("ddagrab=output_idx={monitor}:framerate={fps}:draw_mouse=1,hwdownload,format=bgra");
    vec!["-f".into(), "lavfi".into(), "-i".into(), graph]
}

/// Dos salidas: el video final y un JPEG pequeño que se sobrescribe (preview en vivo).
fn build_args(job: &Job, output: &Path) -> Vec<String> {
    let (_, enc_args, pix) = ENCODERS.iter().find(|(n, _, _)| *n == job.encoder).unwrap_or(&ENCODERS[3]);
    let q = if job.quality == "alta" { "22" } else { "16" };
    let limit: Vec<String> = job.duration.map(|d| vec!["-t".into(), d.to_string()]).unwrap_or_default();

    let mut a: Vec<String> = ["-hide_banner", "-loglevel", "error", "-y"].map(String::from).to_vec();
    a.extend(capture_input(job.monitor, job.fps));
    for dev in &job.audio {
        a.extend(["-f", "dshow", "-thread_queue_size", "1024", "-audio_buffer_size", "50", "-i"].map(String::from));
        a.push(format!("audio={dev}"));
    }

    let mut graph = format!("[0:v]split=2[cap][pv];[cap]format={pix}[v];[pv]fps=4,scale=480:-2,format=yuvj420p[p]");
    let audio_map = match job.audio.len() {
        0 => None,
        1 => Some("1:a".to_string()),
        n => {
            let ins: String = (1..=n).map(|i| format!("[{i}:a]")).collect();
            graph.push_str(&format!(";{ins}amix=inputs={n}:normalize=0[a]"));
            Some("[a]".to_string())
        }
    };
    a.extend(["-filter_complex".into(), graph, "-map".into(), "[v]".into()]);
    if let Some(m) = audio_map {
        a.extend(["-map".into(), m, "-c:a".into(), "aac".into(), "-b:a".into(), "192k".into()]);
    }
    a.extend(["-c:v".into(), job.encoder.clone()]);
    a.extend(enc_args.iter().map(|s| s.replace("{q}", q)));
    a.extend(["-g".into(), (job.fps * 2).to_string()]);
    a.extend(limit.clone());
    a.push(output.to_string_lossy().into());

    a.extend(["-map", "[p]", "-c:v", "mjpeg", "-q:v", "5", "-f", "image2", "-update", "1", "-atomic_writing", "1"].map(String::from));
    a.extend(limit);
    a.push(preview_file().to_string_lossy().into());
    a
}

/// Ejecuta ffmpeg y devuelve un JPEG (en memoria) de como máximo `max_width` px.
fn jpeg(input: &[String], max_width: u32) -> Result<Vec<u8>> {
    let out = hidden(&ffmpeg()?)
        .args(["-hide_banner", "-loglevel", "error"])
        .args(input)
        .args(["-frames:v", "1", "-vf", &format!("scale='min({max_width},iw)':-2"), "-q:v", "4", "-f", "mjpeg", "-"])
        .output()
        .map_err(|e| e.to_string())?;
    if out.stdout.is_empty() {
        return Err(format!("No se pudo generar la imagen: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(out.stdout)
}

// ---------- API pública ----------

pub fn status() -> State {
    let mut st: State = read_json(&state_file());
    let t = now();
    let dead = match st.status.as_str() {
        "recording" | "finishing" => !st.pid.is_some_and(pid_alive),
        "starting" => t - st.started_at.unwrap_or(0.0) > 15.0,
        "idle" => false,
        _ => true,
    };
    if dead {
        let err = (!st.status.is_empty()).then(|| "La grabación terminó inesperadamente".to_string());
        st = State::idle(st.last_file.take(), err);
        write_json(&state_file(), &st);
    }
    if st.status == "recording" {
        st.elapsed_seconds = st.started_at.map(|s| round1(t - s));
    }
    st
}

/// Inicia una grabación en segundo plano. `worker_exe` es el binario que se
/// relanza con [`WORKER_ARG`] para vigilar a ffmpeg.
pub fn start(opts: Options, worker_exe: &Path) -> Result<State> {
    if status().is_active() {
        return Err("Ya hay una grabación en curso".into());
    }
    let cfg = get_config();
    let job = Job {
        fps: opts.fps.unwrap_or(cfg.fps).clamp(1, 240),
        quality: opts.quality.unwrap_or(cfg.quality),
        monitor: opts.monitor.unwrap_or(cfg.monitor),
        audio: opts.audio.unwrap_or(cfg.audio),
        out_dir: opts.out_dir.unwrap_or(cfg.out_dir),
        duration: opts.duration,
        name: opts.name.unwrap_or_else(|| chrono::Local::now().format("grabacion_%Y-%m-%d_%H-%M-%S").to_string()),
        encoder: detect_encoder()?,
    };
    let _ = fs::remove_file(stop_flag());
    write_json(&state_file(), &State { status: "starting".into(), started_at: Some(now()), ..Default::default() });

    detach_std_handles();
    Command::new(worker_exe)
        .arg(WORKER_ARG)
        .arg(serde_json::to_string(&job).map_err(|e| e.to_string())?)
        .creation_flags(DETACHED | NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("No se pudo lanzar la grabación: {e}"))?;

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        sleep(Duration::from_millis(250));
        let st = status();
        match st.status.as_str() {
            "recording" if st.elapsed_seconds.unwrap_or(0.0) >= 1.0 => return Ok(st),
            "idle" => return Err(st.last_error.unwrap_or_else(|| "No se pudo iniciar la grabación".into())),
            _ => {}
        }
    }
    Err("La grabación tardó demasiado en iniciar".into())
}

pub fn wait_until_done(timeout: Duration) -> Result<State> {
    let deadline = Instant::now() + timeout;
    loop {
        let st = status();
        if !st.is_active() {
            return match st.last_error {
                Some(e) => Err(e),
                None => Ok(st),
            };
        }
        if Instant::now() > deadline {
            return Err("Tiempo de espera agotado".into());
        }
        sleep(Duration::from_millis(300));
    }
}

/// Detiene la grabación en curso y espera el MP4 final.
pub fn stop() -> Result<State> {
    if !status().is_active() {
        return Err("No hay ninguna grabación en curso".into());
    }
    let _ = fs::create_dir_all(app_dir());
    File::create(stop_flag()).map_err(|e| e.to_string())?;
    wait_until_done(Duration::from_secs(120))
}

/// Graba `seconds` segundos y bloquea hasta tener el archivo.
pub fn record(seconds: f64, mut opts: Options, worker_exe: &Path) -> Result<State> {
    opts.duration = Some(seconds);
    start(opts, worker_exe)?;
    wait_until_done(Duration::from_secs_f64(seconds + 120.0))
}

/// Captura PNG a resolución completa. Devuelve la ruta.
pub fn screenshot(monitor: Option<u32>) -> Result<String> {
    let cfg = get_config();
    let dir = PathBuf::from(&cfg.out_dir);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(chrono::Local::now().format("captura_%Y-%m-%d_%H-%M-%S.png").to_string());
    let out = hidden(&ffmpeg()?)
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(capture_input(monitor.unwrap_or(cfg.monitor), 10))
        .args(["-frames:v", "1", "-vf", "format=rgb24", "-update", "1"])
        .arg(&path)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() || !path.exists() {
        return Err(format!("No se pudo tomar la captura: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(path.to_string_lossy().into())
}

/// Versión JPEG reducida de una imagen (para mandarla a un agente sin pesar MB).
pub fn preview_jpeg(path: &str, max_width: u32) -> Result<Vec<u8>> {
    jpeg(&["-i".into(), path.into()], max_width)
}

/// Imagen pequeña y actual del monitor (para la vista previa cuando no se graba).
pub fn snapshot(monitor: Option<u32>, max_width: u32) -> Result<Vec<u8>> {
    jpeg(&capture_input(monitor.unwrap_or(get_config().monitor), 10), max_width)
}

/// Último cuadro de la vista previa en vivo, si hay una grabación en curso.
pub fn live_preview() -> Option<Vec<u8>> {
    if status().status != "recording" {
        return None;
    }
    fs::read(preview_file()).ok()
}

pub fn list_recordings(limit: usize) -> Vec<Recording> {
    let Ok(entries) = fs::read_dir(get_config().out_dir) else { return vec![] };
    let mut files: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "mp4" || e == "mkv"))
        .filter_map(|p| Some((fs::metadata(&p).ok()?.modified().ok()?, p)))
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files
        .into_iter()
        .take(limit)
        .map(|(m, p)| Recording {
            size_mb: size_mb(&p),
            modified: chrono::DateTime::<chrono::Local>::from(m).format("%Y-%m-%d %H:%M:%S").to_string(),
            file: p.to_string_lossy().into(),
        })
        .collect()
}

// ---------- proceso de grabación ----------

/// Punto de entrada del worker: lanza ffmpeg, lo detiene con limpieza cuando
/// aparece `stop.flag` y convierte el MKV temporal a MP4.
pub fn run_worker(job_json: &str) {
    let job: Job = match serde_json::from_str(job_json) {
        Ok(j) => j,
        Err(e) => return write_json(&state_file(), &State::idle(None, Some(e.to_string()))),
    };
    if let Err(e) = worker(&job) {
        write_json(&state_file(), &State::idle(None, Some(e)));
    }
}

fn worker(job: &Job) -> Result<()> {
    let ff = ffmpeg()?;
    let dir = PathBuf::from(&job.out_dir);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mkv = dir.join(format!("{}.mkv", job.name)); // MKV aguanta cortes bruscos
    let mp4 = mkv.with_extension("mp4");

    let log = File::create(log_file()).map_err(|e| e.to_string())?;
    let mut child = hidden(&ff)
        .args(build_args(job, &mkv))
        .stdin(Stdio::piped())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log)
        .spawn()
        .map_err(|e| e.to_string())?;

    let started = now();
    let mut state = State {
        status: "recording".into(),
        pid: Some(std::process::id()), // el worker vive hasta terminar el MP4
        started_at: Some(started),
        file: Some(mp4.to_string_lossy().into()),
        fps: Some(job.fps),
        encoder: Some(job.encoder.clone()),
        audio: Some(job.audio.clone()),
        duration: job.duration,
        ..Default::default()
    };
    write_json(&state_file(), &state);
    let border = overlay::Overlay::show(job.monitor);

    let mut stop_sent: Option<Instant> = None;
    while child.try_wait().map_err(|e| e.to_string())?.is_none() {
        if stop_sent.is_none() && stop_flag().exists() {
            let _ = fs::remove_file(stop_flag());
            state.status = "finishing".into();
            write_json(&state_file(), &state);
            if let Some(stdin) = child.stdin.as_mut() {
                let _ = stdin.write_all(b"q").and_then(|_| stdin.flush()); // cierre limpio
            }
            stop_sent = Some(Instant::now());
        }
        if stop_sent.is_some_and(|t| t.elapsed() > Duration::from_secs(15)) {
            let _ = child.kill();
        }
        sleep(Duration::from_millis(200));
    }
    drop(border);
    let _ = fs::remove_file(preview_file());
    let elapsed = round1(now() - started);

    if !fs::metadata(&mkv).is_ok_and(|m| m.len() > 0) {
        let _ = fs::remove_file(&mkv);
        let log = fs::read_to_string(log_file()).unwrap_or_default();
        return Err(format!("ffmpeg falló: {}", log.trim()));
    }

    state.status = "finishing".into();
    write_json(&state_file(), &state);
    let remux_ok = hidden(&ff)
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(&mkv)
        .args(["-c", "copy", "-movflags", "+faststart"])
        .arg(&mp4)
        .status()
        .is_ok_and(|s| s.success());
    let final_file = if remux_ok {
        let _ = fs::remove_file(&mkv);
        mp4
    } else {
        mkv // si falla el paso a MP4, el MKV sigue siendo válido
    };

    let mut done = State::idle(Some(final_file.to_string_lossy().into()), None);
    done.last_size_mb = Some(size_mb(&final_file));
    done.last_duration_seconds = Some(elapsed);
    write_json(&state_file(), &done);
    Ok(())
}
