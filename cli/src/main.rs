//! `grabador`: CLI (salida JSON, pensada para agentes) y servidor MCP por stdio.

mod mcp;

use clap::{Args, Parser, Subcommand};
use grabador_core::{self as core, Options};
use serde::Serialize;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "grabador", version, about = "Grabador de pantalla en alta resolución")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Empieza a grabar en segundo plano
    Start(RecArgs),
    /// Detiene la grabación y guarda el MP4
    Stop,
    /// Estado actual
    Status,
    /// Graba N segundos y espera el archivo
    Record {
        seconds: f64,
        #[command(flatten)]
        rec: RecArgs,
    },
    /// Captura PNG de la pantalla
    Screenshot {
        #[arg(long)]
        monitor: Option<u32>,
    },
    /// Últimas grabaciones
    List {
        #[arg(short, default_value_t = 10)]
        n: usize,
    },
    /// Dispositivos de audio disponibles
    Devices,
    /// Ver o cambiar los valores por defecto
    Config(BaseArgs),
    /// Arranca el servidor MCP (stdio)
    Mcp,
    #[command(name = "__worker", hide = true)]
    Worker { job: String },
}

#[derive(Args)]
struct BaseArgs {
    /// Cuadros por segundo (30 o 60)
    #[arg(long)]
    fps: Option<u32>,
    /// maxima = mejor imagen, alta = archivo más ligero
    #[arg(long, value_parser = ["maxima", "alta"])]
    quality: Option<String>,
    /// Índice del monitor (0 = principal)
    #[arg(long)]
    monitor: Option<u32>,
    /// Dispositivo de audio a incluir (repetible). Ver: grabador devices
    #[arg(long)]
    audio: Vec<String>,
    /// Grabar sin audio
    #[arg(long)]
    no_audio: bool,
    /// Carpeta de salida
    #[arg(long)]
    out: Option<String>,
}

#[derive(Args)]
struct RecArgs {
    #[command(flatten)]
    base: BaseArgs,
    /// Nombre del archivo sin extensión
    #[arg(long)]
    name: Option<String>,
}

impl BaseArgs {
    fn audio(&self) -> Option<Vec<String>> {
        if self.no_audio {
            Some(vec![])
        } else {
            (!self.audio.is_empty()).then(|| self.audio.clone())
        }
    }
}

impl RecArgs {
    fn options(self) -> Options {
        Options {
            audio: self.base.audio(),
            fps: self.base.fps,
            quality: self.base.quality,
            monitor: self.base.monitor,
            out_dir: self.base.out,
            name: self.name,
            duration: None,
        }
    }
}

fn exe() -> PathBuf {
    std::env::current_exe().expect("ruta del ejecutable")
}

fn print<T: Serialize>(result: core::Result<T>) {
    match result {
        Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default()),
        Err(e) => {
            println!("{}", serde_json::json!({ "error": e }));
            std::process::exit(1);
        }
    }
}

fn main() {
    match Cli::parse().cmd {
        Cmd::Start(rec) => print(core::start(rec.options(), &exe())),
        Cmd::Stop => print(core::stop()),
        Cmd::Status => print(Ok(core::status())),
        Cmd::Record { seconds, rec } => print(core::record(seconds, rec.options(), &exe())),
        Cmd::Screenshot { monitor } => print(core::screenshot(monitor).map(|file| serde_json::json!({ "file": file }))),
        Cmd::List { n } => print(Ok(core::list_recordings(n))),
        Cmd::Devices => print(core::audio_devices()),
        Cmd::Config(args) => {
            let mut cfg = core::get_config();
            if let Some(audio) = args.audio() {
                cfg.audio = audio;
            }
            cfg.fps = args.fps.unwrap_or(cfg.fps);
            cfg.quality = args.quality.unwrap_or(cfg.quality);
            cfg.monitor = args.monitor.unwrap_or(cfg.monitor);
            cfg.out_dir = args.out.unwrap_or(cfg.out_dir);
            core::save_config(&cfg);
            print(Ok(cfg));
        }
        Cmd::Mcp => mcp::serve(exe()),
        Cmd::Worker { job } => core::run_worker(&job),
    }
}
