"use strict";
const $ = (s) => document.querySelector(s);
const el = (tag, attrs = {}, ...kids) => {
  const n = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "class") n.className = v;
    else if (k.startsWith("on")) n.addEventListener(k.slice(2), v);
    else if (k.startsWith("aria-") && typeof v === "boolean") n.setAttribute(k, String(v));
    else if (v === true) n.setAttribute(k, "");
    else if (v !== false && v != null) n.setAttribute(k, v);
  }
  for (const k of kids.flat()) if (k != null) n.append(k.nodeType ? k : document.createTextNode(k));
  return n;
};
const icon = (id) => { const s = document.createElementNS("http://www.w3.org/2000/svg", "svg"); s.innerHTML = `<use href="#${id}"/>`; return s; };
const store = {
  get(k, d) { try { const v = localStorage.getItem("tiiny." + k); return v == null ? d : JSON.parse(v); } catch { return d; } },
  set(k, v) { try { localStorage.setItem("tiiny." + k, JSON.stringify(v)); } catch {} },
};
// What a model can do, read from the device's own metadata (supported/input/desc).
function caps(m) {
  if (!m) return { edit: false, maxRefs: 0, transparent: false, annotate: false };
  const desc = m.desc || "";
  const edit = (m.supported || []).includes("Image Editing") || /\bImage\b/.test(m.input || "");
  const refs = Number((desc.match(/up to (\d+) reference images/i) || [])[1]) || 1;
  return {
    edit, maxRefs: edit ? refs : 0,
    transparent: /transparent|alpha channel|RGBA/i.test(desc),
    annotate: edit && /circle|annotation|mask/i.test(desc),
  };
}
const modelById = (id) => (S.allModels || S.models).find((m) => m.id === id);
const canEditModel = (id) => caps(modelById(id)).edit;
const shortName = (id) => (modelById(id)?.display_name || id || "").split("/").pop();
const capsLine = (m) => { const c = caps(m); return ["Create", c.edit && "Edit", c.maxRefs > 1 && `${c.maxRefs} refs`, c.transparent && "Transparent", c.annotate && "Mark area"].filter(Boolean).join(" · "); };

const S = {
  models: [], model: store.get("model", null), mode: "create", sources: [],
  aspect: store.get("aspect", "1:1"), res: store.get("res", 768), n: 1,
  seedLocked: false, transparent: false, history: [], npu: { used: 0, total: 100 }, others: [],
  loading: {}, confirm: null, drawer: false, view: null,  // view: {id, idx} | {job}
  queue: [], running: null, compare: false, deviceOk: null,
  timing: store.get("timing", {}),  // model -> seconds per (megapixel*step-ish unit)
};

const fmtSec = (s) => s < 60 ? `${Math.round(s)}s` : `${Math.floor(s / 60)}m ${String(Math.round(s % 60)).padStart(2, "0")}s`;

/* ---------- toasts ---------- */
function toast(msg, kind = "") {
  const t = el("div", { class: "toast " + kind });
  if (kind === "err") t.append(el("b", {}, "Error · "));
  t.append(msg);
  $("#toasts").append(t);
  setTimeout(() => t.remove(), kind === "err" ? 8000 : 3500);
}

/* ---------- API ---------- */
async function api(path, opts = {}) {
  const r = await fetch(path, { headers: { "Content-Type": "application/json" }, ...opts });
  let j = {}; try { j = await r.json(); } catch {}
  if (!r.ok) { const e = new Error(j?.error?.message || j?.detail || `HTTP ${r.status}`); e.status = r.status; throw e; }
  return j;
}

const KEY_REJECTED = "Your Tiiny rejected the API key. Copy the API key (not the device ID) from TiinyOS: avatar → Settings → API Key.";
async function loadModels(quiet = false) {
  try {
    const j = await api("/api/catalog");
    S.allModels = j.models || []; S.npu = j.npu || S.npu; S.directAccess = j.direct_access;
    if (typeof noteActiveDevice === "function") noteActiveDevice(j.device, j.last_switch);
    S.models = S.allModels.filter((m) => m.workspace === "image");
    S.others = S.allModels.filter((m) => m.workspace !== "image");
    S.deviceOk = true; S.keyRejected = false;
    if (typeof recoverFromDevice === "function") recoverFromDevice();
    for (const id of Object.keys(S.loading)) {
      const m = modelById(id);
      if (m && m.status === "running") {
        delete S.loading[id]; toast(`${shortName(id)} is loaded`);
        if (m.workspace === "image" && !(S.mode === "edit" && !canEditModel(id))) selectModel(id);  // don't kick the user out of an edit
        if (m.type === "Image-to-Text" && S.ocrPending) { const u = S.ocrPending; S.ocrPending = null; ocrImage(u, id); }  // read what was waiting
      }
      // the device lists a load as pending while it works; if it's back to stopped, the Tiiny gave up on it
      else if (m && m.status === "stopped" && Date.now() - S.loading[id] > 30e3) { delete S.loading[id]; toast(`The Tiiny stopped loading ${shortName(id)}. Try again, or unload something first.`, "err"); }
      else if (Date.now() - S.loading[id] > 6 * 60e3) { delete S.loading[id]; toast(`${shortName(id)} didn't finish loading`, "err"); }
    }
    const running = S.models.filter((m) => m.status === "running");
    if (!running.some((m) => m.id === S.model)) {
      S.model = (running.find((m) => m.id === store.get("model", null)) || running.find((m) => canEditModel(m.id)) || running[0] || {}).id || null;
    }
  } catch (e) {
    S.deviceOk = false;
    S.keyRejected = e.status === 401 || e.status === 403;  // the Tiiny answers, but not to this key
    if (!quiet && e.status !== 503) toast(S.keyRejected ? KEY_REJECTED : e.message, "err");  // 503 = not configured; Settings opens itself
  }
  renderModelBtn(); renderDrawer(); renderComposer();
  if (typeof onModelsChanged === "function") onModelsChanged();
}

function timeAgo(t) {
  const s = Date.now() / 1000 - t;
  if (s < 60) return "just now"; if (s < 3600) return Math.floor(s / 60) + " min ago";
  if (s < 86400) return Math.floor(s / 3600) + " h ago";
  return new Date(t * 1000).toLocaleDateString();
}


/* ---------- model button ---------- */
function renderModelBtn() {
  if (S.ws && S.ws !== "image") {
    const run = (S.allModels || []).filter((x) => x.status === "running");
    $("#modelIco").style.visibility = "hidden";
    $("#modelName").textContent = "Models";
    $("#modelCaps").textContent = `${run.length} loaded · ${S.npu.used}% NPU`;
    return;
  }
  const m = modelById(S.model);
  $("#modelIco").src = m ? "/api/icon/" + m.id : "";
  $("#modelIco").style.visibility = m ? "visible" : "hidden";
  $("#modelName").textContent = m ? shortName(m.id) : S.deviceOk === false ? (S.keyRejected ? "API key rejected" : "Device offline") : Object.keys(S.loading).length ? "Loading model…" : "No model loaded";
  $("#modelCaps").textContent = m ? capsLine(m) : "Click to manage models";
}

/* ---------- models drawer ---------- */
const NPU_COLORS = ["#ff7a45", "#60a5fa", "#a78bfa", "#34d399", "#f472b6", "#fbbf24"];
function openDrawer() { if (typeof pollStats === "function") setTimeout(pollStats, 0); S.drawer = true; $("#drawerBody").scrollTop = 0; $("#drawer").classList.add("show"); $("#scrim").classList.add("show"); $("#drawer").setAttribute("aria-hidden", "false"); renderDrawer(); loadModels(true); }
function closeDrawer() { S.drawer = false; S.confirm = null; $("#drawer").classList.remove("show"); $("#scrim").classList.remove("show"); $("#drawer").setAttribute("aria-hidden", "true"); }

function renderDrawer() {
  if (!S.drawer) return;
  const body = $("#drawerBody");
  if (S.deviceOk === false) {
    body.replaceChildren(S.keyRejected
      ? el("div", { class: "empty", style: "margin:40px auto" }, el("h2", {}, "API key rejected"),
        el("p", {}, "Your Tiiny answers, but not to this key. Copy the API key (not the device ID) from TiinyOS: avatar → Settings → API Key."),
        el("button", { class: "btn-sm accent", onclick: () => openSettings(false) }, "Open Settings"))
      : el("div", { class: "empty", style: "margin:40px auto" }, el("h2", {}, "Device offline"), el("p", {}, "Can't reach the Tiiny.")));
    return;
  }

  // NPU budget: everything running, image or not
  const holders = (S.allModels || []).filter((m) => m.status === "running").map((m) => ({ id: m.id, name: shortName(m.id), pct: m.npu_usage || 0 }));
  const pending = Object.keys(S.loading).map((id) => ({ id, name: shortName(id), pct: modelById(id)?.npu_usage || 0, pending: true }));
  const used = holders.reduce((a, h) => a + h.pct, 0);
  const segs = [...holders, ...pending].map((h, i) => ({ ...h, color: NPU_COLORS[i % NPU_COLORS.length] }));
  const busy = (S.jobs || []).some((j) => j.status === "queued" || j.status === "running");
  const unloadAll = holders.length ? el("button", { class: "btn-sm ghost npu-unload", disabled: busy || S.unloadingAll,
    title: busy ? "Wait for running jobs to finish" : "Free all NPU memory", onclick: unloadAllModels }, S.unloadingAll ? "Unloading…" : "Unload all") : null;
  const npu = el("div", { class: "npu" },
    el("div", { class: "npu-top" }, el("span", {}, "NPU memory"), el("span", { class: "npu-right" }, el("span", {}, el("b", {}, `${used}%`), ` used · ${Math.max(0, 100 - used)}% free`), unloadAll)),
    el("div", { class: "npu-bar" }, segs.map((h) => el("i", { class: h.pending ? "pending" : "", style: `width:${h.pct}%;background-color:${h.color}`, title: `${h.name} · ${h.pct}%` }))),
    el("div", { class: "npu-legend" }, segs.length ? segs.map((h) => el("span", {}, el("i", { style: `background:${h.color}` }), `${h.name} ${h.pct}%${h.pending ? " (loading)" : ""}`)) : el("span", {}, "Nothing loaded")));

  const byRun = (a, b) => (b.status === "running") - (a.status === "running");
  const all = S.allModels || [];
  // the current workspace's models first (its workspace names differ from the models' own: mesh = 3d, chat = text)
  const cur = { mesh: "3d", chat: "text" }[S.ws] || S.ws || "image";
  const order = [cur, ...["image", "audio", "3d", "video", "text"].filter((w) => w !== cur)];
  const wsName = { image: "Image", audio: "Audio", "3d": "3D", video: "Video", text: "Chat" };
  const sections = order.map((w) => {
    const list = all.filter((m) => m.workspace === w).sort(byRun);
    return list.length ? [el("div", { class: "section-title" }, `${wsName[w]} models · ${list.length}`), ...list.map(modelCard)] : [];
  }).flat();
  const other = all.filter((m) => m.workspace === "other").sort(byRun);
  const otherBox = other.length ? el("details", { class: "other-models", open: !!S.openOther },
    el("summary", {}, `Other models (embeddings, OCR…) · ${other.length}`),
    ...other.map((o) => el("div", { class: "other-row" },
      el("div", { class: "grow" }, el("div", {}, shortName(o.id), " ", el("span", { class: "status " + (S.loading[o.id] ? "loading" : o.status) }, S.loading[o.id] ? "loading" : o.status === "stopped" ? "not loaded" : o.status)),
        el("small", {}, `${o.type} · ${o.npu_usage ?? "?"}% NPU`)),
      o.status === "running" ? el("button", { class: "btn-sm ghost", onclick: () => stopModel(o.id) }, "Unload")
        : el("button", { class: "btn-sm", disabled: !!S.loading[o.id], onclick: () => loadModel(o.id) }, "Load")))) : null;
  if (otherBox) otherBox.addEventListener("toggle", () => { S.openOther = otherBox.open; });
  body.replaceChildren(...[npu, typeof statsCard === "function" ? statsCard() : null, ...sections, otherBox].filter(Boolean));
}

function modelCard(m) {
  const c = caps(m), active = m.workspace === "image" && m.id === S.model && m.status === "running";
  const loading = S.loading[m.id] != null || m.status === "loading";
  const status = loading ? "loading" : m.status;
  const card = el("div", { class: "mcard" + (active ? " active" : "") });
  card.append(el("div", { class: "mcard-top" },
    el("div", { class: "ico" }, el("img", { src: "/api/icon/" + m.id, alt: "" })),
    el("div", { class: "info" },
      el("div", { class: "nm" }, shortName(m.id), el("span", { class: "status " + status }, active ? "active" : status === "stopped" ? "not loaded" : status)),
      el("div", { class: "sub" }, [m.params, m.type, m.npu_usage != null && `${m.npu_usage}% NPU`].filter(Boolean).join(" · ")))));
  card.append(el("div", { class: "badges" }, m.workspace === "image" ? [
    el("span", { class: "badge-cap on" }, "Create"),
    el("span", { class: "badge-cap " + (c.edit ? "on" : "off") }, "Edit"),
    c.maxRefs > 1 ? el("span", { class: "badge-cap on" }, `Up to ${c.maxRefs} refs`) : null,
    c.transparent ? el("span", { class: "badge-cap on" }, "Transparent PNG") : null,
    c.annotate ? el("span", { class: "badge-cap on" }, "Mark area") : null]
    : (typeof mediaBadges === "function" ? mediaBadges(m) : []).map((b) => el("span", { class: "badge-cap on" }, b))));

  if (loading) {
    const est = m.load_time || 60, t0 = S.loading[m.id] || Date.now();
    const t = (Date.now() - t0) / 1000, f = Math.min(0.95, t / est);
    card.append(el("div", { class: "load-bar" }, el("small", {}, `Loading… ${fmtSec(t)} of ~${fmtSec(est)}`), el("div", { class: "bar" }, el("i", { style: `width:${(f * 100).toFixed(0)}%` }))));
  } else if (S.confirm && S.confirm.id === m.id) {
    const cand = S.confirm.plan.candidate || {};
    const name = (cand.fullname || cand.model_id || "another model").split("/").pop();
    const isActive = cand.model_id === S.model;
    card.append(el("div", { class: "confirm" },
      `${shortName(m.id)} needs ${S.confirm.plan.npu_required}% NPU; only ${S.confirm.plan.npu_available}% is free. Unload `, el("b", {}, name), ` (${cand.npu_usage}%) to make room?`,
      isActive && (S.running || S.queue.length) ? el("div", { style: "margin-top:6px;color:var(--warn)" }, "It's running a job — wait for it to finish first.") : null,
      el("div", { class: "row" },
        el("button", { class: "btn-sm ghost", onclick: () => { S.confirm = null; renderDrawer(); } }, "Cancel"),
        el("button", { class: "btn-sm accent", disabled: isActive && !!(S.running || S.queue.length), onclick: () => confirmUnloadAndLoad(m.id, cand.model_id) }, `Unload ${name} & load`))));
  } else {
    const meta = el("span", { class: "meta" }, m.status === "running" ? (m.busy ? "working…" : "ready") : `loads in ~${fmtSec(m.load_time || 60)}`);
    const acts = [meta];
    if (m.status === "running") {
      if (!active && m.workspace === "image") acts.push(el("button", { class: "btn-sm accent", onclick: () => selectModel(m.id) }, "Use"));
      acts.push(el("button", { class: "btn-sm ghost", disabled: !!(active && (S.running || S.queue.length)), title: active && (S.running || S.queue.length) ? "Busy with a job" : "Free its NPU memory", onclick: () => stopModel(m.id) }, "Unload"));
    } else {
      acts.push(el("button", { class: "btn-sm accent", onclick: () => loadModel(m.id) }, "Load"));
    }
    card.append(el("div", { class: "mcard-actions" }, acts));
  }
  if (m.desc) {
    const d = el("details", { class: "desc", open: !!S.openDesc?.[m.id] }, el("summary", {}, "About this model"), el("p", {}, m.desc.replace(/\*\*/g, "")));
    d.addEventListener("toggle", () => { (S.openDesc ||= {})[m.id] = d.open; });
    card.append(d);
  }
  return card;
}

/* ---------- model control ---------- */
async function unloadAllModels() {
  const names = (S.allModels || []).filter((m) => m.status === "running").map((m) => shortName(m.id));
  if (!confirm(`Unload all ${names.length} model${names.length === 1 ? "" : "s"}?\n\n${names.join("\n")}`)) return;
  S.unloadingAll = true; renderDrawer();
  try { await api("/api/model/unload_all", { method: "POST", body: "{}" }); toast("All models unloaded"); }
  catch (e) { toast(e.message, "err"); }
  S.unloadingAll = false;
  await loadModels(true);
}
async function loadModel(id) {
  if (!S.drawer) openDrawer();
  try {
    const plan = await api("/api/model/plan?id=" + encodeURIComponent(id));
    if (plan.unload_required) { S.confirm = { id, plan }; renderDrawer(); return; }
    await launch(id);
  } catch (e) { toast(e.message, "err"); }
}
async function confirmUnloadAndLoad(id, victim) {
  S.confirm = null;
  try {
    await stopModel(victim, null, true);
    await loadModel(id);  // re-plan: one unload may not be enough
  } catch (e) { toast(e.message, "err"); renderDrawer(); }
}
async function launch(id) {
  S.loading[id] = Date.now(); renderDrawer(); renderModelBtn(); renderComposer();
  try { await api("/api/model/launch", { method: "POST", body: JSON.stringify({ id }) }); }
  catch (e) { delete S.loading[id]; toast(`Couldn't load ${shortName(id)}: ${e.message}`, "err"); }
  loadModels(true);
}
async function stopModel(id, label, silent) {
  try {
    await api("/api/model/stop", { method: "POST", body: JSON.stringify({ id }) });
    if (!silent) toast(`Unloaded ${(label || shortName(id)).split("/").pop()}`);
  } catch (e) { toast(e.message, "err"); if (silent) throw e; }
  await loadModels(true);
}

/* ---------- drop targets ---------- */
function wireDrop(node, take = addFiles) {
  node.addEventListener("dragover", (e) => { e.preventDefault(); node.classList.add("over"); });
  node.addEventListener("dragleave", () => node.classList.remove("over"));
  node.addEventListener("drop", (e) => { e.preventDefault(); e.stopPropagation(); node.classList.remove("over"); $("#dropveil").classList.remove("show"); take(e.dataTransfer.files); });
}

/* ---------- paste targets ----------
   Each place that takes a file registers itself here. Ctrl+V goes to the zone under the pointer, else
   the one last clicked (while it's still on screen), else the workspace's default slot. */
let pasteLast = null;
function fileZone(node, z) {  // z: { key, accept: RegExp, what: "image" | "audio", label: string | () => string, take(files) }
  node.classList.add("paste-zone"); node.dataset.zone = z.key; node._zone = z;
  wireDrop(node, (files) => zoneTake(z, [...files], false));
  node.addEventListener("pointerdown", () => { pasteLast = z.key; });
  // The async clipboard API only hands over images (not audio files), and only on https/localhost.
  if (z.what === "image" && navigator.clipboard?.read) {
    const paste = (e) => { e.preventDefault(); e.stopPropagation(); pasteLast = z.key; clipboardPaste(z); };
    node.append(el("span", { class: "paste-btn", role: "button", tabindex: "0", title: "Paste an image from the clipboard",
      onclick: paste, onkeydown: (e) => { if (e.key === "Enter" || e.key === " ") paste(e); } }, "Paste"));
  }
  return node;
}
function zoneTake(z, files, pasted) {
  const ok = files.filter((f) => z.accept.test(f.type));
  if (!ok.length) { toast(`This spot takes ${z.what === "image" ? "an image" : "audio"}`, "err"); return true; }
  const label = typeof z.label === "function" ? z.label() : z.label;  // read before take() changes the state
  z.take(ok);
  if (pasted) toast("Pasted as " + label);
  return true;
}
async function clipboardPaste(z) {
  try {
    for (const item of await navigator.clipboard.read()) {
      const type = item.types.find((t) => t.startsWith("image/") && z.accept.test(t));
      if (type) return zoneTake(z, [new File([await item.getType(type)], "pasted." + type.split("/")[1], { type })], true);
    }
    toast(`There's no ${z.what} on the clipboard. Copy one first.`, "err");
  } catch (e) {
    if (e.name !== "NotAllowedError") return toast(e.message, "err");
    // the desktop app's window can't read the clipboard itself; the app hands over the picture instead
    const r = await fetch("/api/clipboard-image").catch(() => null);
    if (r?.ok && z.accept.test("image/png")) return zoneTake(z, [new File([await r.blob()], "pasted.png", { type: "image/png" })], true);
    if (r?.status === 404 && r.headers.get("Content-Type")?.includes("json")) {
      const msg = (await r.json()).error?.message;
      if (msg && msg !== "not found") return toast(msg, "err");
    }
    toast("The browser blocked clipboard access. Try Ctrl+V instead.", "err");
  }
}
function pasteToZone(files) {
  const hover = [...document.querySelectorAll(".paste-zone:hover")].pop();
  const last = pasteLast && document.querySelector(`.paste-zone[data-zone="${pasteLast}"]`);
  const node = hover || (last && last.offsetParent !== null ? last : null);
  return node?._zone ? zoneTake(node._zone, files, true) : false;
}

$("#modelBtn").onclick = () => (S.drawer ? closeDrawer() : openDrawer());
$("#drawerClose").onclick = closeDrawer;
$("#scrim").onclick = closeDrawer;
$("#drawerRefresh").onclick = () => loadModels();

// Poll fast while the drawer is open or a model is loading, slowly otherwise.
let lastPoll = Date.now();
setInterval(() => {
  const fast = S.drawer || Object.keys(S.loading).length;
  if (Date.now() - lastPoll < (fast ? 2500 : 30000)) return;
  lastPoll = Date.now(); loadModels(true);
}, 1000);
