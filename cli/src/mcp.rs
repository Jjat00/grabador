//! Servidor MCP mínimo (JSON-RPC 2.0 por stdio, un mensaje por línea).

use base64::Engine;
use grabador_core::{self as core, Options};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

const PROTOCOL: &str = "2025-06-18";

fn rec_props(extra: Value) -> Value {
    let mut props = json!({
        "fps": { "type": "integer", "description": "Frames per second (30 or 60). Default: user setting." },
        "quality": { "type": "string", "enum": ["maxima", "alta"], "description": "maxima = best image, alta = smaller file." },
        "monitor": { "type": "integer", "description": "Monitor index, 0 = primary." },
        "audio": { "type": "array", "items": { "type": "string" },
                   "description": "Audio device names from list_audio_devices. [] = no audio. Omit = user default." },
        "name": { "type": "string", "description": "Output file name without extension." }
    });
    if let (Some(p), Some(e)) = (props.as_object_mut(), extra.as_object()) {
        p.extend(e.clone());
    }
    props
}

fn tools() -> Value {
    json!([
        { "name": "start_recording",
          "description": "Start recording the screen at native resolution in the background (GPU-encoded MP4). Returns immediately; call stop_recording to finish.",
          "inputSchema": { "type": "object", "properties": rec_props(json!({})) } },
        { "name": "stop_recording",
          "description": "Stop the current recording. Returns the final MP4 path in last_file.",
          "inputSchema": { "type": "object", "properties": {} } },
        { "name": "record_clip",
          "description": "Record the screen for a fixed number of seconds and wait until the MP4 is ready.",
          "inputSchema": { "type": "object", "required": ["seconds"],
                           "properties": rec_props(json!({ "seconds": { "type": "number", "description": "Clip length in seconds." } })) } },
        { "name": "recording_status",
          "description": "Current state: idle | starting | recording (with elapsed_seconds) | finishing, plus the last saved file.",
          "inputSchema": { "type": "object", "properties": {} } },
        { "name": "take_screenshot",
          "description": "Save a full-resolution PNG screenshot and return its path (plus a preview image).",
          "inputSchema": { "type": "object", "properties": {
              "monitor": { "type": "integer", "description": "Monitor index, 0 = primary." },
              "include_image": { "type": "boolean", "description": "Attach a 1280px JPEG preview. Default true." } } } },
        { "name": "list_recordings",
          "description": "List the most recent recordings (path, size in MB, date).",
          "inputSchema": { "type": "object", "properties": { "limit": { "type": "integer", "description": "Default 10." } } } },
        { "name": "list_audio_devices",
          "description": "List audio input devices usable in the audio parameter.",
          "inputSchema": { "type": "object", "properties": {} } }
    ])
}

fn text<T: serde::Serialize>(r: core::Result<T>) -> Value {
    match r {
        Ok(v) => json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default() }] }),
        Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
    }
}

fn call(params: &Value, worker: &Path) -> Value {
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    let opts = || serde_json::from_value::<Options>(args.clone()).unwrap_or_default();
    match params["name"].as_str().unwrap_or("") {
        "start_recording" => text(core::start(opts(), worker)),
        "stop_recording" => text(core::stop()),
        "record_clip" => match args["seconds"].as_f64() {
            Some(s) if s > 0.0 => text(core::record(s, opts(), worker)),
            _ => text::<()>(Err("seconds debe ser un número mayor que 0".into())),
        },
        "recording_status" => text(Ok(core::status())),
        "list_recordings" => text(Ok(core::list_recordings(args["limit"].as_u64().unwrap_or(10) as usize))),
        "list_audio_devices" => text(core::audio_devices()),
        "take_screenshot" => {
            let monitor = args["monitor"].as_u64().map(|m| m as u32);
            match core::screenshot(monitor) {
                Err(e) => text::<()>(Err(e)),
                Ok(path) => {
                    let mut content = vec![json!({ "type": "text", "text": format!("Captura guardada en: {path}") })];
                    if args["include_image"].as_bool().unwrap_or(true) {
                        if let Ok(jpg) = core::preview_jpeg(&path, 1280) {
                            let data = base64::engine::general_purpose::STANDARD.encode(jpg);
                            content.push(json!({ "type": "image", "data": data, "mimeType": "image/jpeg" }));
                        }
                    }
                    json!({ "content": content })
                }
            }
        }
        other => text::<()>(Err(format!("Herramienta desconocida: {other}"))),
    }
}

pub fn serve(worker: PathBuf) {
    let mut out = std::io::stdout();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        let Some(id) = msg.get("id").cloned() else { continue }; // notificación: sin respuesta
        let result = match msg["method"].as_str().unwrap_or("") {
            "initialize" => Ok(json!({
                "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or(PROTOCOL),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "grabador", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "Screen recorder for Windows. Use record_clip for fixed-length captures, or start_recording + stop_recording. Files are saved as MP4 at native resolution."
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools() })),
            "tools/call" => Ok(call(&msg["params"], &worker)),
            m => Err(json!({ "code": -32601, "message": format!("Método no soportado: {m}") })),
        };
        let resp = match result {
            Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
            Err(e) => json!({ "jsonrpc": "2.0", "id": id, "error": e }),
        };
        if writeln!(out, "{resp}").and_then(|_| out.flush()).is_err() {
            break;
        }
    }
}
