const { invoke } = window.__TAURI__.core;
const { getCurrentWindow, availableMonitors, LogicalSize } = window.__TAURI__.window;
const win = getCurrentWindow();
const $ = (id) => document.getElementById(id);

const MCP_COMMAND = "claude mcp add grabador -- grabador mcp";
const SYSTEM_HINT = /desktop|stereo|estéreo|mix|loopback|what u hear|virtual audio/i;

let cfg = {};
let state = { status: "idle" };
let monitors = [];
let lastFile;
let busy = false;
let flashTimer;
let previewUrl;
let previewBusy = false;

// ---------- utilidades ----------

const pad = (n) => String(n).padStart(2, "0");
const clock = (s) => `${pad(Math.floor(s / 3600))}:${pad(Math.floor((s % 3600) / 60))}:${pad(Math.floor(s % 60))}`;
const basename = (p) => p.split(/[\\/]/).pop();
const isActive = (s) => ["starting", "recording", "finishing"].includes(s);

function resolution() {
  const m = monitors[cfg.monitor];
  return m ? `${m.size.width}×${m.size.height}` : `${Math.round(screen.width * devicePixelRatio)}×${Math.round(screen.height * devicePixelRatio)}`;
}

// Mensaje temporal en la línea de info: "error" u "ok".
function flash(msg, kind = "error", ms = 5000) {
  $("meta").textContent = msg;
  $("meta").className = `meta ${kind}`;
  clearTimeout(flashTimer);
  flashTimer = setTimeout(() => { $("meta").className = "meta"; render(); }, ms);
}
const showError = (e) => flash(String(e));

async function run(fn) {
  if (busy) return;
  busy = true;
  render();
  try { await fn(); }
  catch (e) { showError(e); }
  finally { busy = false; await poll(); }
}

async function fitWindow() {
  const height = Math.ceil($("app").getBoundingClientRect().height);
  await win.setSize(new LogicalSize(400, height));
}

// ---------- vista previa ----------

// Tauri entrega los bytes como ArrayBuffer o como Array según el comando.
async function showFrame(data) {
  const bytes = new Uint8Array(data ?? []);
  if (!bytes.length) return;
  const img = $("preview");
  const url = URL.createObjectURL(new Blob([bytes], { type: "image/jpeg" }));
  img.src = url;
  await img.decode().catch(() => {});
  if (previewUrl) URL.revokeObjectURL(previewUrl);
  previewUrl = url;
  const ratio = `${img.naturalWidth} / ${img.naturalHeight}`;
  if (img.naturalWidth && $("screen").style.aspectRatio !== ratio) {
    $("screen").style.aspectRatio = ratio;
    fitWindow();
  }
}

async function refreshPreview() {
  if (previewBusy) return;
  previewBusy = true;
  try {
    const buffer = state.status === "recording"
      ? await invoke("live_preview")
      : await invoke("snapshot", { monitor: cfg.monitor });
    await showFrame(buffer);
  } catch { /* sin vista previa: se queda la anterior */ }
  previewBusy = false;
}

// ---------- ajustes ----------

function paintSegments() {
  document.querySelectorAll(".seg").forEach((seg) => {
    seg.querySelectorAll("button").forEach((b) => b.classList.toggle("on", b.dataset.v === String(cfg[seg.dataset.key])));
  });
}

async function saveConfig(patch) {
  cfg = await invoke("set_config", { config: { ...cfg, ...patch } });
  paintSegments();
  render();
  if ("monitor" in patch) refreshPreview();
}

document.querySelectorAll(".seg").forEach((seg) => {
  seg.addEventListener("click", (e) => {
    const v = e.target.closest("button")?.dataset.v;
    if (v === undefined) return;
    const key = seg.dataset.key;
    saveConfig({ [key]: key === "quality" ? v : Number(v) });
  });
});

function fillSelect(el, devices, value) {
  el.innerHTML = "";
  [["", "Ninguno"], ...devices.map((d) => [d, d])].forEach(([v, label]) => el.add(new Option(label, v)));
  el.value = devices.includes(value) ? value : "";
}

async function loadAudio() {
  const devices = await invoke("devices").catch(() => []);
  const sys = cfg.audio.find((a) => SYSTEM_HINT.test(a));
  const mic = cfg.audio.find((a) => a !== sys);
  fillSelect($("sys"), devices, sys);
  fillSelect($("mic"), devices, mic);
}

["sys", "mic"].forEach((id) => $(id).addEventListener("change", () => {
  saveConfig({ audio: [...new Set([$("sys").value, $("mic").value].filter(Boolean))] });
}));

async function loadMonitors() {
  monitors = await availableMonitors().catch(() => []);
  if (monitors.length < 2) return;
  $("monitors").innerHTML = monitors.map((_, i) => `<button data-v="${i}">${i + 1}</button>`).join("");
  $("monitorCell").classList.remove("hidden");
  paintSegments();
}

// ---------- última grabación ----------

async function loadLast() {
  const [r] = await invoke("recordings", { limit: 1 });
  const btn = $("last");
  btn.disabled = !r;
  btn.dataset.file = r?.file ?? "";
  btn.querySelector(".name").textContent = r ? basename(r.file) : "Aún no hay grabaciones";
  btn.querySelector(".size").textContent = r ? `${r.size_mb} MB` : "";
}

$("last").onclick = () => invoke("open_file", { path: $("last").dataset.file }).catch(showError);
$("last").oncontextmenu = (e) => {
  e.preventDefault();
  if ($("last").dataset.file) invoke("reveal", { path: $("last").dataset.file }).catch(showError);
};

// ---------- estado ----------

function render() {
  const s = state.status;
  const active = isActive(s);
  document.body.classList.toggle("rec", s === "recording");
  document.body.classList.toggle("busy", busy || s === "starting" || s === "finishing");
  $("settings").classList.toggle("locked", active || busy);

  const chip = { starting: "Iniciando", recording: "Rec", finishing: "Guardando" }[s] ?? (busy ? "Iniciando" : "Listo");
  $("chip").querySelector("b").textContent = chip;
  $("recText").textContent = active ? "Detener" : "Grabar";
  $("rec").disabled = busy || s === "starting" || s === "finishing";
  $("shot").disabled = busy;
  $("res").textContent = resolution();

  if ($("meta").className === "meta") {
    const audio = (state.audio ?? cfg.audio ?? []).length;
    $("meta").textContent = [
      `${state.fps ?? cfg.fps} FPS`,
      (state.encoder ?? cfg.encoder ?? "H264").toUpperCase(),
      cfg.quality === "alta" ? "LIGERA" : "CALIDAD MÁX",
      audio ? `AUDIO ×${audio}` : "SIN AUDIO",
    ].join(" · ");
  }
  if (!active) $("timer").textContent = clock(0);
}

function tick() {
  if (state.status === "recording" && state.started_at) {
    $("timer").textContent = clock(Math.max(0, Date.now() / 1000 - state.started_at));
  }
  requestAnimationFrame(tick);
}

async function poll() {
  const prev = state.status;
  try { state = await invoke("status"); } catch { return; }
  if (state.last_error && !isActive(state.status) && !busy) showError(state.last_error);
  if (state.last_file !== lastFile) {
    lastFile = state.last_file;
    loadLast();
  }
  if (prev === "recording" && state.status !== "recording") refreshPreview();
  render();
}

// ---------- acciones ----------

$("rec").onclick = () => run(() => invoke(isActive(state.status) ? "stop" : "start"));
$("shot").onclick = () => run(async () => flash(`✓ ${basename(await invoke("screenshot"))}`, "ok", 3000));
$("folder").onclick = () => invoke("open_folder").catch(showError);
$("mcp").onclick = async () => {
  await navigator.clipboard.writeText(MCP_COMMAND);
  flash(`✓ Copiado: ${MCP_COMMAND}`, "ok", 3500);
};

$("min").onclick = () => win.minimize();
$("close").onclick = () => win.close();
let pinned = false;
$("pin").onclick = async () => {
  pinned = !pinned;
  await win.setAlwaysOnTop(pinned);
  $("pin").classList.toggle("on", pinned);
};
window.addEventListener("focus", () => state.status !== "recording" && refreshPreview());

// ---------- inicio ----------

(async () => {
  cfg = await invoke("get_config");
  paintSegments();
  await Promise.all([loadAudio(), loadMonitors(), poll()]);
  await refreshPreview();
  await fitWindow();
  await win.center();
  await win.show();
  setInterval(poll, 500);
  setInterval(() => state.status === "recording" && refreshPreview(), 250);
  tick();
})();
