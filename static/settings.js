"use strict";
/* Device settings: saved Tiinys (name, address, API key) with auto-switching between them.
   Keys are stored on the server and never sent back to the browser. */

const SET = { info: null, editing: null, checking: false };

async function loadSettings() {
  try { SET.info = await api("/api/settings"); } catch { SET.info = null; }
  renderDeviceLabel();
  if (SET.info && !SET.info.configured) openSettings(true);
  return SET.info;
}
function renderDeviceLabel() {
  const i = SET.info;
  $("#deviceHost").textContent = i?.host ? `Tiiny · ${i.name || i.host}` : "not connected";
  $("#deviceHost").title = i?.host ? `${i.url}${i.auto && i.devices?.length > 1 ? " · auto-switch on" : ""}` : "";
}
/** Called from the model poll with the device the server is using; tells the user about switches. */
function noteActiveDevice(dev, lastSwitch) {
  if (!dev || !SET.info) return;
  if (dev.id !== SET.info.active) {
    SET.info.active = dev.id; SET.info.name = dev.name; SET.info.host = dev.host;
    renderDeviceLabel();
    if (lastSwitch && Date.now() / 1000 - lastSwitch.t < 120) toast(`Switched to ${lastSwitch.to} — ${lastSwitch.reason}`);
    if ($("#settingsModal").classList.contains("show") && !SET.editing) refreshSettings();
  }
}

const HELP = () => el("details", { class: "set-help" },
  el("summary", {}, "Where do I find these?"),
  el("ol", {},
    el("li", {}, el("b", {}, "API key: "), "in TiinyOS, click your avatar (lower-left) → Settings → API Key, then the copy icon next to the key."),
    el("li", {}, el("b", {}, "Address: "), "avatar → Settings → Networks shows the Tiiny's IP (turn on Wi-Fi first under Settings → Wi-Fi). Your router's or phone hotspot's device list works too."),
    el("li", {}, el("b", {}, "USB cable: "), "the Tiiny is the other address on the cable's tiny network, e.g. 172.20.31.109 if your PC is …110. Separate song tracks and GLM-OCR need Wi-Fi/LAN" + (SET.info?.app ? "." : ", and Docker can't reach a 172.17.x.x USB address (see the guide)."))),
  el("p", {}, "Don't use the ", el("code", {}, "openai.api.tiiny"), " URLs from that page: they only work on the PC running TiinyOS. ",
    el("a", { href: "/docs/CONNECTING.md", target: "_blank", rel: "noopener" }, "Full guide")));

/** Add/edit form for one device. dev = existing device or null for a new one. */
function deviceForm(dev, firstRun) {
  const name = el("input", { class: "input", placeholder: "e.g. Home Wi-Fi, Phone hotspot, USB cable", value: dev?.name || "", autocomplete: "off" });
  const url = el("input", { class: "input", placeholder: "http://192.168.1.50", value: dev?.url || "", autocomplete: "off", spellcheck: "false" });
  const key = el("input", { class: "input", type: "password", autocomplete: "off", spellcheck: "false",
    placeholder: dev?.key_set ? `saved (${dev.key_hint || "hidden"}) — leave blank to keep` : "paste your device API key" });
  const status = el("div", { class: "set-status", "aria-live": "polite" });
  const show = el("button", { class: "btn-sm ghost", type: "button", onclick: () => { key.type = key.type === "password" ? "text" : "password"; show.textContent = key.type === "password" ? "Show" : "Hide"; } }, "Show");
  const keyNote = el("small", { class: "set-note" });
  url.oninput = () => {
    const moved = dev && url.value.trim() && !url.value.includes(dev.host);
    keyNote.textContent = moved && !key.value ? "New address: enter the key again (a saved key is only ever sent to its own device)." : "";
  };
  const test = async () => {
    status.className = "set-status"; status.textContent = "Testing…";
    try {
      const r = await api("/api/settings/test", { method: "POST", body: JSON.stringify({ id: dev?.id, url: url.value, key: key.value }) });
      status.className = "set-status " + (r.ok ? "ok" : "bad"); status.textContent = (r.ok ? "✓ " : "✕ ") + r.message;
    } catch (e) { status.className = "set-status bad"; status.textContent = "✕ " + e.message; }
  };
  const save = async () => {
    try {
      const body = { name: name.value, url: url.value, key: key.value };
      if (dev) body.id = dev.id; else if (!firstRun) body.new = true;
      SET.info = await api("/api/settings", { method: "POST", body: JSON.stringify(body) });
      const saved = dev ? SET.info.devices.find((d) => d.id === dev.id) : SET.info.devices[SET.info.devices.length - 1];
      if (saved && !saved.key_set) { status.className = "set-status bad"; status.textContent = "Saved the address — now enter the API key."; key.focus(); return; }
      SET.editing = null;
      renderDeviceLabel();
      toast(dev ? "Device updated" : "Device saved");
      if (firstRun) { closeSettings(); loadModels(); loadHistory(); if (typeof pollJobs === "function") pollJobs(); }
      else { refreshSettings(); loadModels(true); }
    } catch (e) { status.className = "set-status bad"; status.textContent = "✕ " + e.message; }
  };
  setTimeout(() => (dev ? key : firstRun ? url : name).focus(), 0);
  return el("form", { class: "set-form", onsubmit: (e) => { e.preventDefault(); save(); } },
    firstRun ? el("p", { class: "set-intro" }, "Tiiny Studio talks to your device over your network. Enter its address and API key once; they're stored " + (SET.info?.app ? "by the app on this computer" : "on this server, and the key never goes back to the browser") + ". You can add more addresses later (home Wi-Fi, hotspot, USB) and the studio switches to whichever one answers.") : null,
    el("label", { class: "field" }, el("div", { class: "label" }, "Name", el("span", { class: "hint" }, "optional")), name),
    el("label", { class: "field" }, el("div", { class: "label" }, "Device address", el("span", { class: "hint" }, "IP or hostname")), url),
    el("label", { class: "field" }, el("div", { class: "label" }, "API key", el("span", { class: "hint" }, "from the TiinyOS app")),
      el("div", { class: "seed-row" }, key, show), keyNote),
    HELP(),
    status,
    el("div", { class: "row set-actions" },
      el("button", { class: "btn-sm", type: "button", onclick: test }, "Test connection"),
      el("span", { class: "spacer" }),
      firstRun ? null : el("button", { class: "btn-sm ghost", type: "button", onclick: () => { SET.editing = null; refreshSettings(); } }, "Cancel"),
      el("button", { class: "btn-sm accent", type: "submit" }, dev ? "Save" : "Add device")));
}

function deviceList() {
  const i = SET.info || {};
  const act = async (path, body) => { try { SET.info = await api("/api/settings/" + path, { method: "POST", body: JSON.stringify(body) }); renderDeviceLabel(); refreshSettings(); if (path === "activate") loadModels(true); } catch (e) { toast(e.message, "err"); } };
  const rows = (i.devices || []).map((d, n) => {
    const st = d.status;
    const dot = el("span", { class: "dev-dot " + (st ? (st.ok ? "ok" : "bad") : ""), title: st ? (st.ok ? `answering · ${st.ms} ms` : "not answering") : "not checked yet" });
    const active = d.id === i.active;
    return el("div", { class: "dev-row" + (active ? " active" : "") },
      dot,
      el("div", { class: "grow" },
        el("div", { class: "dev-name" }, d.name, n === 0 && i.devices.length > 1 ? el("span", { class: "dev-tag" }, "preferred") : null, active ? el("span", { class: "dev-tag on" }, "in use") : null),
        el("small", {}, `${d.host}${d.key_set ? ` · key ${d.key_hint}` : " · no key"}${st?.ok ? ` · ${st.ms} ms` : st ? " · not answering" : ""}`)),
      n > 0 ? el("button", { class: "icon-btn sm", type: "button", title: "Move up (higher priority)", onclick: () => act("move", { id: d.id, dir: -1 }) }, "↑") : null,
      active ? null : el("button", { class: "btn-sm", type: "button", disabled: !d.key_set, onclick: () => act("activate", { id: d.id }) }, "Use"),
      el("button", { class: "btn-sm ghost", type: "button", onclick: () => { SET.editing = d.id; refreshSettings(); } }, "Edit"),
      i.devices.length > 1 ? el("button", { class: "btn-sm ghost dev-del", type: "button", title: "Remove", onclick: () => { if (confirm(`Remove “${d.name}” and its saved key?`)) act("delete", { id: d.id }); } }, "✕") : null);
  });
  const auto = el("label", { class: "auto dev-auto" },
    el("input", { type: "checkbox", checked: !!i.auto, onchange: (e) => act("auto", { auto: e.target.checked }) }),
    "Switch automatically to the first device in the list that answers (a device you pick with Use stays until it stops answering)");
  return el("div", { class: "set-devices" },
    el("div", { class: "label" }, "Devices", el("span", { class: "hint" }, el("a", { href: "#", onclick: async (e) => { e.preventDefault(); SET.checking = true; refreshSettings(); await act("check", {}); SET.checking = false; refreshSettings(); } }, SET.checking ? "checking…" : "check now"))),
    ...rows,
    el("div", { class: "row", style: "margin-top:10px;flex-wrap:wrap;gap:10px" },
      el("button", { class: "btn-sm", type: "button", onclick: () => { SET.editing = "new"; refreshSettings(); } }, "+ Add device"),
      i.devices?.length > 1 ? auto : null));
}

/** Model swapping: a global switch and one per section (only counts while the global one is on), plus the
    text model helpers use. Stored on the server, which enforces them. */
function swapSettings() {
  const i = SET.info || {}, sw = i.swap || {};
  const set = async (body) => { try { SET.info = await api("/api/settings/swap", { method: "POST", body: JSON.stringify(body) }); refreshSettings(); } catch (e) { toast(e.message, "err"); } };
  const box = (k, label) => el("label", { class: "auto" + (sw.on ? "" : " off") },
    el("input", { type: "checkbox", checked: sw[k] !== false, disabled: !sw.on, onchange: (e) => set({ swap: { [k]: e.target.checked } }) }), label);
  const chats = (S.allModels || []).filter((m) => CHAT_TYPES.includes(m.type));
  const pick = el("select", { class: "input", onchange: (e) => set({ helper_model: e.target.value }) },
    el("option", { value: "" }, "Automatic: a loaded chat model, else the smallest"),
    ...chats.map((m) => el("option", { value: m.id, selected: m.id === i.helper_model }, `${shortName(m.id)} · ${m.npu_usage ?? "?"}% NPU`)));
  return el("div", { class: "set-network set-swap" },
    el("b", {}, "Model swapping"),
    el("p", {}, "When a helper or a chat tool needs a model that isn't loaded, the studio unloads what's in the way, loads it, and puts your models back afterwards. Each swap takes about 30–60 seconds per model. Off: nothing is loaded or unloaded for you; you're asked to load the model instead."),
    el("label", { class: "auto" }, el("input", { type: "checkbox", checked: sw.on !== false, onchange: (e) => set({ swap: { on: e.target.checked } }) }), "Let the studio swap models for me"),
    el("div", { class: "swap-secs" }, box("chat", "Chat tools (images, speech, loops, songs from a chat)"), box("image", "Image helpers (Improve prompt, Describe)"), box("audio", "Audio helpers (Write lyrics, loop tags)"), box("mesh", "3D (cut out the subject with an image model)")),
    el("label", { class: "field" }, el("div", { class: "label" }, "Helpers use", el("span", { class: "hint" }, "a text model")), pick));
}

function settingsBody(firstRun) {
  const i = SET.info || {};
  if (firstRun) return [deviceForm(null, true)];
  if (SET.editing) return [deviceForm(SET.editing === "new" ? null : i.devices.find((d) => d.id === SET.editing), false)];
  return [
    deviceList(),
    i.configured ? swapSettings() : null,
    el("div", { class: "set-privacy" },
      el("div", {}, el("b", {}, "Privacy"),
        el("p", {}, "What you make is saved only on this computer, in the studio's data folder, and the studio sends it nowhere but your Tiiny. Deleting a result removes it and its prompt, sources and cached copies. To clear it all at once:"),
        el("p", { class: "set-privacy-device" }, "Your Tiiny keeps its own copy: since TiinyOS 0.1.36 its vault stores every chat it receives as full text (from any app, this studio included), whatever the memory scope setting. TiinyOS doesn't show it; a day can be deleted through the device's API. ",
          el("a", { href: "/docs/CONNECTING.md", target: "_blank", rel: "noopener" }, "What your Tiiny keeps"))),
      el("button", { class: "btn-sm danger-btn", type: "button", onclick: wipeEverything }, "Delete everything…")),
    i.configured ? el("div", { class: "set-privacy" },
      el("div", {}, el("b", {}, "Power"), el("p", {}, `Shut the Tiiny down${i.name ? ` (via ${i.name})` : ""} the same way the TiinyOS app does. To turn it back on, press its power button.`)),
      el("button", { class: "btn-sm danger-btn", type: "button", onclick: powerOffDevice }, "Power off…")) : null,
    // the desktop app only serves its own window and has no .env: just say so
    i.app ? el("div", { class: "set-network" }, el("b", {}, "Network: this computer only"), el("p", {}, "The app's studio only answers this window; nobody else on your network can open it.")) :
    el("div", { class: "set-network" + (i.lan_access && !i.password ? " warn" : "") },
      el("b", {}, i.lan_access ? "Network: open to your LAN" : "Network: this computer only"),
      el("p", {}, i.lan_access
        ? (i.password ? "Other devices on your network can open the studio with the password." : "Other devices on your network can open the studio — and no password is set. Add STUDIO_PASSWORD to .env, or remove STUDIO_BIND to go back to this-computer-only.")
        : "Nobody else on your network can open it. To allow phones or other computers, set STUDIO_BIND=0.0.0.0 (and a STUDIO_PASSWORD) in .env and restart.")),
    el("div", { class: "row set-actions" },
      el("a", { class: "set-credit", href: "https://github.com/thezolon", target: "_blank", rel: "noopener" }, `Tiiny Studio ${SET.info?.version || ""} · by Zolon Farsane`.replace("  ", " ")),
      el("span", { class: "spacer" }),
      el("button", { class: "btn-sm accent", type: "button", onclick: closeSettings }, "Done")),
  ];
}

function openSettings(firstRun = false) {
  SET.editing = null;
  SET.firstRun = firstRun;
  const card = el("div", { class: "modal-card set-card" },
    el("div", { class: "modal-head" }, el("h3", {}, firstRun ? "Connect your Tiiny" : "Device settings"), el("span", { class: "spacer" }),
      firstRun ? null : el("button", { class: "icon-btn", title: "Close (Esc)", onclick: closeSettings }, "✕")),
    el("div", { class: "set-body", id: "setBody" }, ...settingsBody(firstRun)));
  $("#settingsModal").replaceChildren(card);
  $("#settingsModal").classList.add("show");
  $("#settingsModal").dataset.locked = firstRun ? "1" : "";
  if (!firstRun) api("/api/settings/check", { method: "POST", body: "{}" }).then((r) => { SET.info = r; if (!SET.editing) refreshSettings(); }).catch(() => {});
}
function refreshSettings() {
  const body = $("#setBody");
  if (body) body.replaceChildren(...settingsBody(SET.firstRun && !SET.info?.configured));
}
async function wipeEverything() {
  const n = S.history.length;
  if (!confirm(`Permanently delete everything the studio has saved?\n\n• ${n} result${n === 1 ? "" : "s"} (images, audio, 3D, transcripts) and their prompts\n• edit sources, saved voices and saved chats\n\nDevice settings are kept. Your Tiiny's own request records aren't affected. This can't be undone.`)) return;
  try {
    const r = await api("/api/privacy/wipe", { method: "POST", body: "{}" });
    S.history = []; S.view = null;
    for (const k of Object.keys(localStorage)) if (k.startsWith("tiiny.") && !/^tiiny\.(ws|model|aspect|res|timing|mesh\.|audio\.tool|audio\.lang)/.test(k)) { try { localStorage.removeItem(k); } catch {} }
    if (typeof A !== "undefined") { A.view = null; A.voices = []; A.speech.voiceId = null; }
    if (typeof M !== "undefined") M.view = null;
    closeSettings(); renderAll();
    for (const k of ["audio", "mesh"]) { const m = WS[k].mod(); if (m.mounted) { k === "audio" ? m.loadVoices() : m.render(); } }
    toast(`Deleted ${r.entries} results, ${r.voices} voices and ${r.files} files`);
  } catch (e) { toast(e.message, "err"); }
}

async function powerOffDevice() {
  const i = SET.info || {};
  const busy = (S.jobs || []).filter((j) => j.status === "queued" || j.status === "running").length;
  if (busy) return toast(`${busy} job${busy > 1 ? "s are" : " is"} still running or queued. Power off after ${busy > 1 ? "they finish" : "it finishes"}.`, "err");
  if (!confirm(`Power off the Tiiny at ${i.host || i.url}${i.name ? ` (${i.name})` : ""}?\n\nEvery loaded model stops and the device shuts down within a few seconds. Nothing in the studio is lost.\n\nTo turn it back on, press the Tiiny's power button.`)) return;
  try {
    await api("/api/device/shutdown", { method: "POST", body: "{}" });
    toast("The Tiiny is shutting down");
    closeSettings();
    setTimeout(() => loadModels(true), 8000);  // the Models panel then shows it as unreachable
    if (!(await deviceGoesOff())) toast("The Tiiny said it would power off but it's still on. Press its power button to turn it off.", "err");
  } catch (e) { toast(e.message, "err"); }
}

/** The device answers "ok" even when its shutdown fails (TiinyOS 0.1.36), so check it actually went away:
    true once it stops answering, false if it's still up after a minute. */
async function deviceGoesOff() {
  const until = Date.now() + 60000;
  while (Date.now() < until) {
    await new Promise((r) => setTimeout(r, 5000));
    try { await api("/api/models"); } catch { return true; }
  }
  return false;
}

function closeSettings() { if ($("#settingsModal").dataset.locked === "1" && !SET.info?.configured) return; SET.editing = null; $("#settingsModal").classList.remove("show"); }

$("#settingsBtn").onclick = () => openSettings(false);
document.addEventListener("keydown", (e) => { if (e.key === "Escape" && $("#settingsModal").classList.contains("show")) { e.stopPropagation(); closeSettings(); } }, true);
document.addEventListener("DOMContentLoaded", loadSettings);
