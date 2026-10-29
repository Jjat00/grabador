const { invoke } = window.__TAURI__.core;
const { getCurrentWindow, availableMonitors, LogicalSize } = window.__TAURI__.window;
const win = getCurrentWindow();
const $ = (id) => document.getElementById(id);

const NONE = "";
const SYSTEM_HINT = /desktop|stereo|estéreo|mix|loopback|what u hear|virtual audio/i;

let cfg = {};
let state = { status: "idle" };
let lastFile = null;
let busy = false;
let flashTimer;

// ---------- utilidades ----------

const pad = (n) => String(n).padStart(2, "0");
const clock = (s) => `${pad(Math.floor(s / 3600))}:${pad(Math.floor((s % 3600) / 60))}:${pad(Math.floor(s % 60))}`;
const basename = (p) => p.split(/[\\/]/).pop();
const isActive = (s) => ["starting", "recording", "finishing"].includes(s);
const nativeRes = () => `${Math.round(screen.width * devicePixelRatio)}×${Math.round(screen.height * devicePixelRatio)}`;

// Mensaje temporal en la línea de info: "error" o "ok".
function flash(msg, kind = "error", ms = 5000) {
  const meta = $("meta");
  meta.textContent = msg;
  meta.className = `meta ${kind}`;
  clearTimeout(flashTimer);
  flashTimer = setTimeout(() => { meta.className = "meta"; render(); }, ms);
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
  [[NONE, "Ninguno"], ...devices.map((d) => [d, d])].forEach(([v, label]) => el.add(new Option(label, v)));
  el.value = devices.includes(value) ? value : NONE;
}

async function loadAudio() {
  const devices = await invoke("devices").catch(() => []);
  const sys = cfg.audio.find((a) => SYSTEM_HINT.test(a));
  const mic = cfg.audio.find((a) => a !== sys);
  fillSelect($("sys"), devices, sys);
  fillSelect($("mic"), devices, mic);
}

["sys", "mic"].forEach((id) => $(id).addEventListener("change", () => {
  const audio = [...new Set([$("sys").value, $("mic").value].filter(Boolean))];
  saveConfig({ audio });
}));

async function loadMonitors() {
  const monitors = await availableMonitors().catch(() => []);
  if (monitors.length < 2) return;
  $("monitors").innerHTML = monitors.map((_, i) => `<button data-v="${i}">${i + 1}</button>`).join("");
  $("monitorCell").classList.remove("hidden");
  paintSegments();
}

// ---------- recientes ----------

async function loadRecent() {
  const list = await invoke("recordings", { limit: 2 });
  const ul = $("recent");
  if (!list.length) {
    ul.innerHTML = `<li class="empty">Aún no hay grabaciones</li>`;
    return;
  }
  ul.innerHTML = "";
  list.forEach((r) => {
    const li = document.createElement("li");
    li.title = "Clic: reproducir · Clic derecho: mostrar en carpeta";
    li.innerHTML = `<span class="name"></span><span class="size">${r.size_mb} MB</span><span class="go">↗</span>`;
    li.querySelector(".name").textContent = basename(r.file);
    li.onclick = () => invoke("open_file", { path: r.file }).catch(showError);
    li.oncontextmenu = (e) => { e.preventDefault(); invoke("reveal", { path: r.file }).catch(showError); };
    ul.append(li);
  });
}

// ---------- estado ----------

function render() {
  const s = state.status;
  const active = isActive(s);
  document.body.classList.toggle("rec", s === "recording");
  document.body.classList.toggle("busy", busy || s === "starting" || s === "finishing");
  $("settings").classList.toggle("locked", active || busy);

  const chip = { idle: "Listo", starting: "Iniciando", recording: "Rec", finishing: "Guardando" }[s] || "Listo";
  $("chip").querySelector("b").textContent = busy && !active ? "Iniciando" : chip;
  $("recText").textContent = active ? "Detener" : "Grabar";
  $("rec").disabled = busy || s === "starting" || s === "finishing";
  $("shot").disabled = busy;

  if ($("meta").className === "meta") {
    const fps = state.fps ?? cfg.fps;
    const audio = (state.audio ?? cfg.audio ?? []).length;
    const parts = [nativeRes(), `${fps} FPS`, (state.encoder ?? cfg.encoder ?? "H264").toUpperCase()];
    parts.push(audio ? `AUDIO ×${audio}` : "SIN AUDIO");
    $("meta").textContent = parts.join(" · ");
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
  try { state = await invoke("status"); } catch { return; }
  if (state.last_error && !isActive(state.status) && !busy) showError(state.last_error);
  if (state.last_file !== lastFile) {
    lastFile = state.last_file;
    loadRecent();
  }
  render();
}

// ---------- acciones ----------

$("rec").onclick = () => run(() => invoke(isActive(state.status) ? "stop" : "start"));
$("shot").onclick = () => run(async () => {
  const file = await invoke("screenshot");
  flash(`✓ ${basename(file)}`, "ok", 3000);
});
$("folder").onclick = () => invoke("open_folder").catch(showError);
$("copy").onclick = async () => {
  await navigator.clipboard.writeText($("mcpCmd").textContent);
  $("copy").textContent = "Copiado";
  setTimeout(() => ($("copy").textContent = "Copiar"), 1500);
};

$("min").onclick = () => win.minimize();
$("close").onclick = () => win.close();
let pinned = false;
$("pin").onclick = async () => {
  pinned = !pinned;
  await win.setAlwaysOnTop(pinned);
  $("pin").classList.toggle("on", pinned);
};

// ---------- inicio ----------

(async () => {
  cfg = await invoke("get_config");
  paintSegments();
  await Promise.all([loadAudio(), loadMonitors(), poll()]);
  const height = Math.ceil(document.getElementById("app").getBoundingClientRect().height);
  await win.setSize(new LogicalSize(400, height));
  await win.center();
  await win.show();
  setInterval(poll, 500);
  tick();
})();
