// Fenêtre de contrôle : affiche l'écran distant et transmet clavier/souris.
const { invoke, Channel } = window.__TAURI__.core;
const appWindow = window.__TAURI__.window.getCurrentWindow();

const $ = (id) => document.getElementById(id);
const sid = Number(new URLSearchParams(location.search).get("sid"));
const canvas = $("screen");
const ctx = canvas.getContext("2d", { alpha: false });
const stage = $("stage");

const QUALITY_LABELS = { 1: "Rapide", 2: "Équilibrée", 3: "Haute" };
const OS_LABELS = { windows: "Windows", macos: "macOS", linux: "Linux" };
const isMac = /Mac/i.test(navigator.platform || navigator.userAgent);

let info = null;
let fit = true;
let closed = false;
let gotFirstFrame = false;
let swapCtrlMeta = false;

// ── Images ──────────────────────────────────────────────────────────────
// Chaque message binaire contient une image : en-tête puis tuiles JPEG.
// On les dessine dans l'ordre, puis on accuse réception (contrôle de flux).

const queue = [];
let drawing = false;
const onFrame = new Channel();
onFrame.onmessage = (buffer) => {
  queue.push(buffer);
  if (!drawing) pump();
};

async function pump() {
  drawing = true;
  while (queue.length) await drawFrame(queue.shift());
  drawing = false;
}

async function drawFrame(buffer) {
  const view = new DataView(buffer);
  const seq = view.getUint32(0);
  if (closed) return; // session finie : on garde la dernière image figée
  try {
    const width = view.getUint16(4);
    const height = view.getUint16(6);
    const count = view.getUint16(8);
    if (canvas.width !== width || canvas.height !== height) {
      canvas.width = width;
      canvas.height = height;
      layout();
    }
    let offset = 10;
    const jobs = [];
    for (let i = 0; i < count; i++) {
      const x = view.getUint16(offset);
      const y = view.getUint16(offset + 2);
      const length = view.getUint32(offset + 8);
      offset += 12;
      const blob = new Blob([new Uint8Array(buffer, offset, length)], { type: "image/jpeg" });
      offset += length;
      jobs.push(createImageBitmap(blob).then((bitmap) => ({ x, y, bitmap })));
    }
    for (const { x, y, bitmap } of await Promise.all(jobs)) {
      ctx.drawImage(bitmap, x, y);
      bitmap.close();
    }
    stats.frame(buffer.byteLength);
    if (!gotFirstFrame && !closed) {
      gotFirstFrame = true;
      document.body.classList.add("has-frame");
      hideOverlay();
      canvas.focus();
    }
  } catch (err) {
    console.error("image illisible", err);
  } finally {
    invoke("viewer_ack", { sid, seq }).catch(() => {});
  }
}

const stats = {
  frames: 0,
  bytes: 0,
  frame(size) {
    this.frames += 1;
    this.bytes += size;
  },
};
setInterval(() => {
  if (!gotFirstFrame || closed) return;
  const mb = stats.bytes / 1e6;
  const rate = mb >= 1 ? `${mb.toFixed(1).replace(".", ",")} Mo/s` : `${Math.round(stats.bytes / 1e3)} Ko/s`;
  $("stats").textContent = `${stats.frames} i/s · ${rate}`;
  stats.frames = 0;
  stats.bytes = 0;
}, 1000);

// ── Mise à l'échelle ────────────────────────────────────────────────────

function layout() {
  if (!canvas.width) return;
  stage.classList.toggle("native", !fit);
  if (fit) {
    const scale = Math.min(stage.clientWidth / canvas.width, stage.clientHeight / canvas.height);
    canvas.style.width = `${Math.floor(canvas.width * scale)}px`;
    canvas.style.height = `${Math.floor(canvas.height * scale)}px`;
  } else {
    // 1:1 : un pixel distant = un pixel de l'écran local.
    canvas.style.width = `${canvas.width / devicePixelRatio}px`;
    canvas.style.height = `${canvas.height / devicePixelRatio}px`;
  }
}
new ResizeObserver(layout).observe(stage);

// ── Souris ──────────────────────────────────────────────────────────────

function send(event) {
  if (!closed) invoke("viewer_input", { sid, event }).catch(() => {});
}

const clamp01 = (v) => Math.min(1, Math.max(0, v));
function position(e) {
  const r = canvas.getBoundingClientRect();
  return { x: clamp01((e.clientX - r.left) / r.width), y: clamp01((e.clientY - r.top) / r.height) };
}

// Un seul déplacement envoyé par image affichée.
let pendingMove = null;
let moveQueued = false;
function queueMove(p) {
  pendingMove = p;
  if (!moveQueued) {
    moveQueued = true;
    requestAnimationFrame(flushMove);
  }
}
function flushMove() {
  moveQueued = false;
  if (pendingMove) {
    send({ kind: "mouse_move", ...pendingMove });
    pendingMove = null;
  }
}

const BUTTONS = ["left", "middle", "right", "back", "forward"];
const pressedButtons = new Set();

canvas.addEventListener("mousedown", (e) => {
  e.preventDefault();
  canvas.focus();
  const button = BUTTONS[e.button];
  if (!button || closed) return;
  pendingMove = position(e);
  flushMove();
  pressedButtons.add(button);
  send({ kind: "mouse_button", button, down: true });
});

window.addEventListener("mouseup", (e) => {
  const button = BUTTONS[e.button];
  if (!button || !pressedButtons.has(button)) return;
  e.preventDefault();
  pendingMove = position(e);
  flushMove();
  pressedButtons.delete(button);
  send({ kind: "mouse_button", button, down: false });
});

window.addEventListener("mousemove", (e) => {
  // Pendant un glisser, on suit la souris même hors de l'image.
  if (e.target === canvas || pressedButtons.size) queueMove(position(e));
});

canvas.addEventListener("contextmenu", (e) => e.preventDefault());
canvas.addEventListener("auxclick", (e) => e.preventDefault());

// Molette : unités de 1/120 de cran, regroupées par image affichée.
let wheelX = 0;
let wheelY = 0;
let wheelQueued = false;
canvas.addEventListener(
  "wheel",
  (e) => {
    e.preventDefault();
    const k = e.deltaMode === 1 ? 40 : e.deltaMode === 2 ? 800 : 1.2;
    wheelX += e.deltaX * k;
    wheelY += e.deltaY * k;
    if (wheelQueued) return;
    wheelQueued = true;
    requestAnimationFrame(() => {
      wheelQueued = false;
      const dx = Math.trunc(wheelX);
      const dy = Math.trunc(wheelY);
      wheelX -= dx;
      wheelY -= dy;
      if (dx || dy) send({ kind: "wheel", dx, dy });
    });
  },
  { passive: false },
);

// ── Clavier ─────────────────────────────────────────────────────────────
// On transmet la touche physique (KeyboardEvent.code) : le poste distant
// applique sa propre disposition, comme un clavier branché dessus.

const SWAP = { MetaLeft: "ControlLeft", MetaRight: "ControlRight", ControlLeft: "MetaLeft", ControlRight: "MetaRight" };
const mapCode = (code) => (swapCtrlMeta && SWAP[code]) || code;
const pressedKeys = new Set();
const fromChrome = (e) => e.target instanceof Element && e.target.closest("#toolbar, #overlay");

window.addEventListener("keydown", (e) => {
  if (closed || fromChrome(e) || !e.code || e.code === "Unidentified") return;
  e.preventDefault();
  const code = mapCode(e.code);
  pressedKeys.add(code);
  send({ kind: "key", code, key: e.key, down: true });
});

window.addEventListener("keyup", (e) => {
  if (closed || !e.code || e.code === "Unidentified") return;
  const code = mapCode(e.code);
  if (fromChrome(e) && !pressedKeys.has(code)) return;
  e.preventDefault();
  pressedKeys.delete(code);
  send({ kind: "key", code, key: e.key, down: false });
});

// Fenêtre quittée (Alt+Tab…) : rien ne doit rester enfoncé à distance.
function releaseAll() {
  if (pressedKeys.size || pressedButtons.size) send({ kind: "release_all" });
  pressedKeys.clear();
  pressedButtons.clear();
}
window.addEventListener("blur", releaseAll);

// ── Barre d'outils ──────────────────────────────────────────────────────

$("monitor").addEventListener("change", (e) => {
  invoke("viewer_set_monitor", { sid, index: Number(e.target.value) }).catch(console.error);
  canvas.focus();
});

$("quality").addEventListener("change", (e) => {
  invoke("viewer_set_quality", { sid, quality: Number(e.target.value) }).catch(console.error);
  canvas.focus();
});

$("fit").addEventListener("click", () => {
  fit = !fit;
  $("fit").setAttribute("aria-pressed", String(!fit));
  $("fit-label").textContent = fit ? "Ajusté" : "1:1";
  layout();
  canvas.focus();
});

$("fullscreen").addEventListener("click", async () => {
  const next = !(await appWindow.isFullscreen());
  await appWindow.setFullscreen(next);
  document.body.classList.toggle("fullscreen", next);
  $("fullscreen").setAttribute("aria-pressed", String(next));
  canvas.focus();
});

$("disconnect").addEventListener("click", async () => {
  await invoke("viewer_disconnect", { sid }).catch(() => {});
  appWindow.close();
});

$("overlay-close").addEventListener("click", () => appWindow.close());

// ── Superposition (connexion, fin de session) ───────────────────────────

function showOverlay(title, text, final) {
  $("overlay-title").textContent = title;
  $("overlay-text").textContent = text;
  $("overlay").hidden = false;
  $("overlay").classList.toggle("final", Boolean(final));
  $("overlay-close").hidden = !final;
  if (final) $("overlay-close").focus();
}

function hideOverlay() {
  $("overlay").hidden = true;
}

// ── Démarrage ───────────────────────────────────────────────────────────

const onEvent = new Channel();
onEvent.onmessage = (event) => {
  if (event.type !== "closed" || closed) return;
  releaseAll();
  closed = true;
  document.body.classList.add("ended");
  $("stats").textContent = "Déconnecté";
  showOverlay("Session terminée", event.reason, true);
};

async function start() {
  try {
    info = await invoke("viewer_attach", { sid, onFrame, onEvent });
  } catch (err) {
    closed = true;
    showOverlay("Session indisponible", String(err), true);
    return;
  }
  swapCtrlMeta = isMac !== (info.os === "macos");
  $("host-name").textContent = info.hostname;
  $("host-meta").textContent = `${info.hostId} · ${OS_LABELS[info.os] ?? info.os}`;
  $("overlay-text").textContent = `Réception de l'écran de ${info.hostname}…`;

  const monitor = $("monitor");
  monitor.replaceChildren(
    ...info.monitors.map((m, i) => {
      const label = `${i + 1}. ${m.name}${m.primary ? " (principal)" : ""}`;
      return new Option(label, String(i), false, i === info.monitor);
    }),
  );
  $("monitor-field").hidden = info.monitors.length < 2;

  const quality = $("quality");
  quality.replaceChildren(
    ...Object.entries(QUALITY_LABELS).map(([value, label]) => new Option(label, value, false, Number(value) === info.quality)),
  );
}

start();
