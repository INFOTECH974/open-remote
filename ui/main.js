// Fenêtre principale : ce poste (ID, mot de passe, réglages) et connexion sortante.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);
const OS_LABELS = { windows: "Windows", macos: "macOS", linux: "Linux" };
const STATUS_LABELS = { online: "En ligne", connecting: "Connexion", error: "Hors ligne", disabled: "Désactivé" };

let overview = null;

// ── Affichage ───────────────────────────────────────────────────────────

async function refresh() {
  overview = await invoke("get_overview");
  render();
}

function render() {
  const o = overview;
  $("hostname").textContent = o.hostname;
  $("my-id").textContent = o.id;
  $("my-password").textContent = groupPassword(o.password);
  $("accept").checked = o.acceptIncoming;
  $("unattended").checked = o.unattended;
  $("unattended-edit").hidden = !o.unattended;
  $("autostart").checked = o.autostart;
  $("version").textContent = `v${o.version}`;
  document.body.classList.toggle("host-off", !o.acceptIncoming);
  renderStatus(o.status);
  renderMachines(o.machines);
  renderIncoming(o.incoming);
}

const groupPassword = (pw) => pw.replace(/(.{4})(?=.)/g, "$1 ");

function renderStatus(status) {
  const lamp = $("relay-lamp");
  lamp.dataset.state = status.state;
  $("relay-state").textContent = STATUS_LABELS[status.state] ?? status.state;
  $("relay-addr").textContent = overview?.relay ?? "";
  $("status-text").textContent = status.message;
}

function renderIncoming(incoming) {
  const active = Boolean(incoming && incoming.active);
  $("incoming").hidden = !active;
  document.body.classList.toggle("controlled", active);
  if (active) {
    $("incoming-text").textContent = `${incoming.name} (${OS_LABELS[incoming.os] ?? incoming.os}) contrôle ce poste.`;
  }
}

function relativeTime(seconds) {
  const diff = Date.now() / 1000 - seconds;
  if (diff < 60) return "à l'instant";
  if (diff < 3600) return `il y a ${Math.floor(diff / 60)} min`;
  if (diff < 86400) return `il y a ${Math.floor(diff / 3600)} h`;
  if (diff < 172800) return "hier";
  return new Date(seconds * 1000).toLocaleDateString("fr-FR", { day: "numeric", month: "short" });
}

function renderMachines(machines) {
  $("machines-empty").hidden = machines.length > 0;
  $("machines").replaceChildren(
    ...machines.map((m) => {
      const row = $("machine-row").content.firstElementChild.cloneNode(true);
      row.querySelector(".m-name").textContent = m.name;
      row.querySelector(".m-id").textContent = m.displayId;
      row.querySelector(".m-os").textContent = OS_LABELS[m.os] ?? m.os;
      row.querySelector(".m-last").textContent = relativeTime(m.last);
      row.classList.toggle("saved", m.saved);
      const open = row.querySelector(".m-open");
      open.textContent = m.saved ? "Connexion" : "Choisir";
      open.classList.toggle("primary", m.saved);
      open.title = m.saved ? `Se connecter à ${m.name}` : "Remplir l'ID, puis saisir le mot de passe";
      open.addEventListener("click", () => (m.saved ? connectSaved(m) : pick(m)));
      const forgetButton = row.querySelector(".m-forget");
      forgetButton.title = `Retirer ${m.name} de la liste`;
      forgetButton.addEventListener("click", () => forget(m));
      return row;
    }),
  );
}

// ── Notifications : restent affichées tant qu'on ne les a pas lues ──────

function pushNotice(message, kind) {
  const notice = $("notice");
  const time = new Date().toLocaleTimeString("fr-FR", { hour: "2-digit", minute: "2-digit" });
  $("notice-text").textContent = `${message} (${time})`;
  // « info » (fichier reçu) : même bandeau, sans le rouge réservé aux refus.
  notice.classList.toggle("info", kind === "info");
  notice.hidden = false;
  document.body.classList.add("has-notice");
}

$("notice-dismiss").addEventListener("click", () => {
  $("notice").hidden = true;
  document.body.classList.remove("has-notice");
});

// ── Ce poste ────────────────────────────────────────────────────────────

async function copy(text, button) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const area = Object.assign(document.createElement("textarea"), { value: text });
    document.body.append(area);
    area.select();
    document.execCommand("copy");
    area.remove();
  }
  const title = button.title;
  button.title = "Copié";
  button.classList.add("done");
  setTimeout(() => {
    button.title = title;
    button.classList.remove("done");
  }, 1400);
}

$("copy-id").addEventListener("click", (e) => copy(overview.id.replace(/\s/g, ""), e.currentTarget));
$("copy-password").addEventListener("click", (e) => copy(overview.password, e.currentTarget));

$("new-password").addEventListener("click", async () => {
  overview.password = await invoke("regenerate_password");
  $("my-password").textContent = groupPassword(overview.password);
});

$("accept").addEventListener("change", async (e) => {
  await invoke("set_accept_incoming", { enabled: e.target.checked }).catch(pushNotice);
  await refresh();
});

$("autostart").addEventListener("change", async (e) => {
  await invoke("set_autostart", { enabled: e.target.checked }).catch(pushNotice);
  await refresh();
});

// Accès sans surveillance : définir, modifier ou retirer le mot de passe permanent.
const unattendedDialog = $("unattended-dialog");

$("unattended").addEventListener("change", async (e) => {
  if (e.target.checked) {
    e.target.checked = false; // validé seulement après enregistrement
    openUnattended();
  } else {
    await invoke("set_unattended_password", { password: null }).catch(pushNotice);
    await refresh();
  }
});
$("unattended-edit").addEventListener("click", openUnattended);

function openUnattended() {
  $("unattended-form").reset();
  $("unattended-error").textContent = "";
  unattendedDialog.showModal();
  $("perm-password").focus();
}

$("unattended-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const password = $("perm-password").value;
  if (password !== $("perm-confirm").value) {
    $("unattended-error").textContent = "Les deux mots de passe ne correspondent pas.";
    return;
  }
  const button = $("unattended-save");
  button.disabled = true;
  try {
    await invoke("set_unattended_password", { password });
    unattendedDialog.close();
    await refresh();
  } catch (err) {
    $("unattended-error").textContent = String(err);
  } finally {
    button.disabled = false;
  }
});
$("unattended-cancel").addEventListener("click", () => unattendedDialog.close());

// Réglages du relais.
const settingsDialog = $("settings-dialog");
$("open-settings").addEventListener("click", () => {
  $("relay").value = overview.relay;
  $("relay-key").value = overview.relayKey;
  $("settings-error").textContent = "";
  settingsDialog.showModal();
  $("relay").focus();
});
$("settings-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  try {
    await invoke("save_settings", { relay: $("relay").value, relayKey: $("relay-key").value });
    settingsDialog.close();
    await refresh();
  } catch (err) {
    $("settings-error").textContent = String(err);
  }
});
$("settings-cancel").addEventListener("click", () => settingsDialog.close());

$("end-incoming").addEventListener("click", () => invoke("end_incoming"));

// ── Contrôler un poste ──────────────────────────────────────────────────

const peerId = $("peer-id");
peerId.addEventListener("input", () => {
  const digits = peerId.value.replace(/\D/g, "").slice(0, 9);
  peerId.value = digits.replace(/(\d{3})(?=\d)/g, "$1 ");
});

function setBusy(busy) {
  document.body.classList.toggle("connecting", busy);
  $("connect").disabled = busy;
  $("connect-label").textContent = busy ? "Connexion…" : "Se connecter";
  for (const b of document.querySelectorAll(".m-open")) b.disabled = busy;
}

function setFormMessage(text) {
  $("form-message").textContent = text;
  $("form-message").hidden = !text;
}

async function run(action) {
  setFormMessage("");
  setBusy(true);
  try {
    await action();
    await refresh();
  } catch (err) {
    setFormMessage(String(err));
  } finally {
    setBusy(false);
  }
}

$("connect-form").addEventListener("submit", (e) => {
  e.preventDefault();
  run(async () => {
    await invoke("connect", {
      id: peerId.value,
      password: $("peer-password").value,
      remember: $("remember").checked,
    });
    $("peer-password").value = "";
  });
});

function connectSaved(machine) {
  peerId.value = machine.displayId;
  run(async () => {
    try {
      await invoke("connect_saved", { id: machine.id });
    } catch (err) {
      // Mot de passe changé sur le poste distant : on propose de le ressaisir.
      $("peer-password").focus();
      throw err;
    }
  });
}

function pick(machine) {
  peerId.value = machine.displayId;
  $("peer-password").focus();
}

async function forget(machine) {
  await invoke("forget_machine", { id: machine.id }).catch(pushNotice);
  await refresh();
}

// ── Événements venant de Rust ───────────────────────────────────────────

listen("host-status", () => refresh());
listen("incoming-session", (e) => renderIncoming(e.payload));
listen("host-notice", (e) => pushNotice(e.payload.message, e.payload.kind));

// Les « il y a 5 min » vieillissent avec la fenêtre.
setInterval(() => overview && renderMachines(overview.machines), 60_000);

refresh();
