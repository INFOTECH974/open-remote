// Fenêtre de contrôle : affiche l'écran distant, transmet clavier/souris et
// envoie des fichiers au poste distant.
const { invoke, Channel } = window.__TAURI__.core;
const appWindow = window.__TAURI__.window.getCurrentWindow();
const webview = window.__TAURI__.webviewWindow.getCurrentWebviewWindow();

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
const fromChrome = (e) =>
  e.target instanceof Element && e.target.closest("#toolbar, #overlay, #transfers, #host-warning");

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

// ── Envoi de fichiers ───────────────────────────────────────────────────
// Par le bouton de la barre d'outils ou en déposant des fichiers dans la
// fenêtre. Ils partent un par un et arrivent dans le dossier Téléchargements
// du poste distant.

const transfers = new Map(); // id → { id, name, size, state, row, timer }
const ACTIVE = new Set(["queued", "sending"]);
// Un petit fichier peut être confirmé avant que sa ligne ne soit affichée.
let sendsInFlight = 0;
let earlyEvents = [];

function formatSize(bytes) {
  if (bytes < 1000) return `${bytes} octet${bytes > 1 ? "s" : ""}`;
  const units = ["Ko", "Mo", "Go", "To"];
  let value = bytes / 1000;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit += 1;
  }
  const digits = value < 10 ? 1 : 0;
  return `${value.toLocaleString("fr-FR", { minimumFractionDigits: digits, maximumFractionDigits: digits })} ${units[unit]}`;
}

function setTransfer(t, state, text, ratio) {
  t.state = state;
  t.row.dataset.state = state;
  t.row.querySelector(".transfer-meta").textContent = text;
  if (ratio !== undefined) t.row.querySelector(".transfer-bar span").style.transform = `scaleX(${ratio})`;
  const button = t.row.querySelector(".transfer-dismiss");
  button.title = ACTIVE.has(state) ? "Annuler l'envoi" : "Retirer de la liste";
  button.setAttribute("aria-label", `${button.title} : ${t.name}`);
}

function addTransfer(file) {
  const row = $("transfer-template").content.firstElementChild.cloneNode(true);
  const t = { id: file.id, name: file.name, size: file.size, state: "", row, timer: 0 };
  const name = row.querySelector(".transfer-name");
  name.textContent = file.name;
  name.title = file.name;
  row.querySelector(".transfer-dismiss").addEventListener("click", () => {
    dismissTransfer(t);
    canvas.focus();
  });
  transfers.set(t.id, t);
  $("transfer-list").append(row);
  if (file.error) setTransfer(t, "error", file.error);
  else setTransfer(t, "queued", `En attente · ${formatSize(file.size)}`, 0);
  $("transfers").hidden = false;
}

function dismissTransfer(t) {
  if (ACTIVE.has(t.state) && !closed) invoke("viewer_cancel_file", { sid, id: t.id }).catch(() => {});
  clearTimeout(t.timer);
  t.row.remove();
  transfers.delete(t.id);
  $("transfers").hidden = transfers.size === 0;
}

function onFileEvent(event) {
  const t = transfers.get(event.id);
  if (!t) {
    if (sendsInFlight) earlyEvents.push(event);
    return;
  }
  if (event.type === "file_progress") {
    const ratio = t.size ? Math.min(1, event.sent / t.size) : 1;
    const percent = Math.floor(ratio * 100);
    setTransfer(t, "sending", `${formatSize(event.sent)} sur ${formatSize(t.size)} · ${percent} %`, ratio);
  } else if (event.type === "file_sent") {
    setTransfer(t, "saving", "Enregistrement sur le poste distant…", 1);
  } else if (event.type === "file_done") {
    // Espaces insécables : un guillemet ne reste jamais seul en fin de ligne.
    const renamed = event.name !== t.name ? ` sous le nom « ${event.name} »` : "";
    setTransfer(t, "done", `Reçu dans Téléchargements${renamed}`, 1);
    // Une réussite s'efface seule ; une erreur reste jusqu'à ce qu'on la retire.
    t.timer = setTimeout(() => dismissTransfer(t), 8000);
  } else if (event.type === "file_error") {
    setTransfer(t, "error", event.message);
  }
}

async function sendFiles(paths) {
  if (closed || !info || !paths || !paths.length) return;
  sendsInFlight += 1;
  try {
    const files = await invoke("viewer_send_files", { sid, paths });
    files.forEach(addTransfer);
  } catch (err) {
    console.error("envoi impossible", err);
  } finally {
    sendsInFlight -= 1;
    const events = earlyEvents;
    earlyEvents = [];
    events.forEach(onFileEvent);
  }
}

$("send-file").addEventListener("click", async () => {
  if (closed) return;
  try {
    await sendFiles(await invoke("viewer_pick_files"));
  } catch (err) {
    console.error("sélection de fichiers impossible", err);
  }
  canvas.focus();
});

webview.onDragDropEvent((event) => {
  const { type, paths } = event.payload;
  const hovering = (type === "enter" || type === "over") && !closed && Boolean(info);
  document.body.classList.toggle("dropping", hovering);
  if (type === "drop") sendFiles(paths);
});

// ── Autorisations manquantes sur le poste distant ───────────────────────
// Un Mac qui n'a pas autorisé Distant envoie un écran vide ou ignore la
// souris et le clavier : il le signale, pour qu'on ne cherche pas ailleurs.

function showHostWarning(warnings) {
  const screen = warnings.includes("screen_capture");
  const input = warnings.includes("accessibility");
  if (!screen && !input) return;
  const title = screen
    ? "Ce poste n'a pas autorisé l'enregistrement de l'écran"
    : "Ce poste n'a pas autorisé le contrôle de la souris et du clavier";
  const effects = [];
  if (screen) effects.push("L'image restera vide.");
  if (input) effects.push("La souris et le clavier resteront sans effet.");
  $("host-warning-title").textContent = title;
  $("host-warning-detail").textContent =
    `${effects.join(" ")} Sur le poste distant, ouvrez Distant : il indique les autorisations à accorder dans les Réglages Système, puis relancez-le.`;
  $("host-warning").hidden = false;
}

$("host-warning-dismiss").addEventListener("click", () => {
  $("host-warning").hidden = true;
  canvas.focus();
});

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
  if (event.type !== "closed") {
    onFileEvent(event);
    return;
  }
  if (closed) return;
  releaseAll();
  closed = true;
  document.body.classList.add("ended");
  document.body.classList.remove("dropping");
  $("send-file").disabled = true;
  $("stats").textContent = "Déconnecté";
  for (const t of transfers.values()) {
    if (t.state !== "done" && t.state !== "error") setTransfer(t, "error", "Session terminée avant la fin de l'envoi.");
  }
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
  showHostWarning(info.warnings ?? []);
  $("host-name").textContent = info.hostname;
  $("transfers-host").textContent = info.hostname;
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
