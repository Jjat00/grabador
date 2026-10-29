# Grabador

Grabador de pantalla para Windows en alta resolución: app de escritorio minimalista, CLI y servidor MCP para agentes.

- Resolución nativa, 30/60 FPS, codificación por GPU (NVENC, Quick Sync o AMF; si no hay, x264) → MP4.
- Vista previa en vivo dentro de la app, también antes de grabar.
- Modo **Auto**: graba (y previsualiza) el monitor donde está el puntero; o fija uno.
- Recuadro rojo sutil alrededor del monitor que se graba; **no sale en el video** (`WDA_EXCLUDEFROMCAPTURE`).
- Audio del sistema y micrófono, mezclados.
- App, CLI y MCP comparten estado: lo que inicia uno lo puede detener otro.

Requiere [ffmpeg](https://ffmpeg.org) en el `PATH` (`winget install ffmpeg`).

## Estructura

| Carpeta | Qué es |
|---|---|
| `core/` | Motor de grabación (ffmpeg + `ddagrab`), estado compartido y recuadro de grabación |
| `cli/` | Binario `grabador`: CLI con salida JSON y servidor MCP por stdio |
| `src-tauri/` + `ui/` | App de escritorio (Tauri 2, HTML/CSS/JS sin bundler) |

## Instalar

```sh
cargo install --path cli                  # CLI + MCP → ~/.cargo/bin/grabador.exe
npx @tauri-apps/cli@2 build               # instalador en target/release/bundle/nsis/
```

## CLI

```sh
grabador start [--fps 60] [--quality maxima|alta] [--monitor 0] [--audio "Dispositivo"]... [--no-audio] [--name x]
grabador stop                 # detiene y devuelve la ruta del MP4
grabador record 10            # graba 10 s y espera el archivo
grabador status
grabador screenshot
grabador list [-n 10]
grabador devices              # dispositivos de audio
grabador monitors             # monitores y el que se grabaría ahora
grabador config [--fps ...] [--monitor 1 | --follow-mouse]   # valores por defecto
```

## MCP

```sh
claude mcp add grabador -s user -- grabador mcp
```

Herramientas: `start_recording`, `stop_recording`, `record_clip`, `recording_status`, `take_screenshot`, `list_recordings`, `list_audio_devices`, `list_monitors`.

Las grabaciones se guardan en `~/Videos/Grabador`; el estado y la configuración, en `%LOCALAPPDATA%\tech.jaimeaza.grabador`.
