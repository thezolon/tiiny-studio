"use strict";
/* ---------- model store: browse, download (one at a time), pause, cancel, updates, delete ----------
   Two tabs in the Models drawer: Installed (studio.js draws the cards; this adds the updates area and each card's
   Manage row) and Store. Requested on the Tiiny Discord. The servers refuse anything unsafe (a loaded or busy model,
   a second download, too little space); this file explains it before the user gets there. Text from the device
   (descriptions, update notes) only ever goes in as text. */

const GB = (b) => (typeof b === "number" && b > 0 ? `${(b / 1e9).toFixed(b < 1e9 ? 2 : 1)} GB` : "size unknown");
const SPACE_FLOOR_GB = 2;  // the servers keep this much free after a download
const UNDER_WAY = ["downloading", "paused", "error"];
Object.assign(S, {
  mtab: store.get("modelsTab", "installed"), store: null, storeErr: null, downloads: [], storeQuery: "", storeWs: store.get("storeWs", "all"),
  hideInstalled: store.get("storeHideInstalled", false), mconfirm: null, dlSamples: {}, dlSeen: {}, updateQueue: [],
  missing: store.get("missingModels", []), mmBusy: {},
  checking: new Set(),   // downloads that left the list: finished, cancelled or failed? the store says which
  cancelled: new Set(),  // cancelled from here: no "finished" toast when they disappear
  updateIds: new Set(),  // downloads that are updates: their old build is already deleted
  updating: null,        // {id, since}: the update Update all (or one Update) waits for, set once its request succeeded
  pollSeq: 0, pollShown: 0,
});

const storeModel = (id) => (S.store?.models || []).find((m) => m.id === id);
const mmName = (id) => storeModel(id)?.name || shortName(id);
const busyOnDevice = (id) => { const m = modelById(id); return !!(m && (m.status === "running" || m.status === "loading" || S.loading[id])); };
const jobOn = (id) => (S.jobs || []).some((j) => j.model === id && (j.status === "queued" || j.status === "running"));
const nowDownloading = () => (S.downloads.find((d) => d.status === "downloading") || {}).id || null;  // fresh every poll
const updateRunning = () => !!(S.updating || S.updateQueue.length || S.updateInFlight);
function saveMissing() { store.set("missingModels", S.missing); }

/* ---------- polling ---------- */
// Both polls feed noteDownloads; an answer that arrives after a newer one is dropped, so nothing is announced twice.
async function loadStore() {
  const seq = ++S.pollSeq;
  let j;
  try { j = await api("/api/store"); }
  catch (e) { S.storeErr = e.message; renderDrawer(); return; }  // the catalog's own poll explains an offline or locked Tiiny
  if (seq < S.pollShown) return;
  S.pollShown = seq; S.store = j; S.storeErr = null;
  noteDownloads(j.models.filter((m) => UNDER_WAY.includes(m.status)).map((m) => ({ id: m.id, name: m.name, status: m.status, progress: m.progress, size: m.size })), j.models);
  if (!j.store_error) {  // a model stays "missing" while the store lists it as not downloaded
    S.missing = S.missing.filter((id) => storeModel(id)?.status === "not_downloaded");
    saveMissing();
  }
  renderDrawer();
}
async function loadDownloads() {
  const seq = ++S.pollSeq;
  let j;
  try { j = await api("/api/downloads"); } catch { return; }
  if (seq < S.pollShown) return;
  S.pollShown = seq;
  noteDownloads(j.downloads || []);
}
/** Compare with the last poll. A download that left the list is checked against the store before anything is said. */
function noteDownloads(list, models) {
  const before = S.dlSeen, now = {};
  for (const d of list) {
    now[d.id] = d.status;
    if (d.status === "error" && before[d.id] !== "error") toast(`The download of ${d.name || mmName(d.id)} failed. Does the Tiiny have internet (Wi-Fi)?`, "err");
    if (d.status === "downloading" && typeof d.progress === "number") {
      const s = (S.dlSamples[d.id] ||= []); s.push([Date.now(), d.progress]);
      while (s.length > 2 && Date.now() - s[0][0] > 180e3) s.shift();
    } else delete S.dlSamples[d.id];
  }
  for (const id of Object.keys(before)) if (!now[id]) S.checking.add(id);
  const u = S.updating;
  // a quick update may finish between two polls; the clock starts when its request succeeded
  if (u && !now[u.id] && Date.now() - u.since > 4000) S.checking.add(u.id);
  if (u && now[u.id] === "error") { S.updating = null; stopQueue(); }  // its download failed: the rest stay as they were
  S.downloads = list; S.dlSeen = now;
  if (S.checking.size) { if (models) settle(models, now); else loadStore(); }
  renderModelBtn();
}
function settle(models, now) {
  const done = [];
  for (const id of [...S.checking]) {
    const m = models.find((x) => x.id === id);
    if (now[id]) { S.checking.delete(id); continue; }  // back in the list: still under way
    S.checking.delete(id);
    const wasUpdate = S.updateIds.has(id);
    S.updateIds.delete(id); delete S.dlSamples[id];
    if (S.updating?.id === id) S.updating = null;
    if (m?.installed) { done.push(id); continue; }
    if (S.cancelled.delete(id)) continue;  // the user stopped it: they know
    toast(`${mmName(id)} stopped downloading and isn't on your Tiiny.`, "err");
    if (wasUpdate && !S.missing.includes(id)) { S.missing.push(id); saveMissing(); }
    stopQueue();
  }
  if (done.length) {
    for (const id of done) announce(`${mmName(id)}: download finished`);
    toast(done.length === 1 ? `${mmName(done[0])} is on your Tiiny` : `${done.length} downloads finished`);
    loadModels(true);
  }
  nextUpdate();
}
function stopQueue() {
  if (!S.updateQueue.length) return;
  S.updateQueue = [];
  toast("Update all stopped; the rest were left as they were.", "err");
}
// Fast while the drawer is open (the catalog poll in studio.js runs then too), every 5 s while a download runs.
let lastStorePoll = 0;
setInterval(() => {
  const open = S.drawer, active = S.downloads.some((d) => d.status === "downloading") || updateRunning() || S.checking.size;
  const every = open ? 2500 : active ? 5000 : 0;
  if (!every || Date.now() - lastStorePoll < every || S.deviceOk === false) return;
  lastStorePoll = Date.now();
  (open || S.checking.size ? loadStore : loadDownloads)();
}, 1000);
loadDownloads();  // a download started earlier (or in the TiinyOS app) shows from the start

/* ---------- the header button: "↓ 34%" while a download runs ---------- */
function dlBadge() {
  const d = S.downloads.find((x) => x.status === "downloading");
  if (!d || S.deviceOk === false) return;
  $("#modelCaps").textContent = `Downloading ${d.name || mmName(d.id)} · ${Math.floor(d.progress || 0)}%`;
}

/* ---------- drawer: tabs, downloads strip ---------- */
function mmTabs() {
  const tab = (id, label) => el("button", { role: "tab", id: "mmTab-" + id, "aria-selected": S.mtab === id, "aria-controls": "mmPanel",
    tabindex: S.mtab === id ? 0 : -1, onclick: () => setMtab(id) }, label);
  const updates = S.store?.updates || 0;
  const list = el("div", { class: "mm-tabs", role: "tablist", "aria-label": "Models" },
    tab("installed", updates ? `Installed · ${updates} update${updates === 1 ? "" : "s"}` : "Installed"), tab("store", "Model Store"));
  list.addEventListener("keydown", (e) => {
    const to = { ArrowRight: "store", ArrowLeft: "installed", Home: "installed", End: "store" }[e.key];
    if (to) { e.preventDefault(); setMtab(to); $("#mmTab-" + to)?.focus(); }
  });
  return list;
}
/** Open the Models drawer on the Model Store, filtered to a workspace (a tab's "no model installed" note uses this). */
function openStore(ws) {
  if (ws) { S.storeWs = ws; store.set("storeWs", ws); }
  S.hideInstalled = false;
  if (!S.drawer) openDrawer();
  setMtab("store");
}
function setMtab(id) {
  S.mtab = id; S.mconfirm = null; store.set("modelsTab", id);
  $("#drawerBody").scrollTop = 0; renderDrawer();
  if (id === "store" && !S.store) loadStore();
}

function eta(id, p) {
  const s = S.dlSamples[id] || [];
  if (s.length < 2 || s[s.length - 1][0] - s[0][0] < 60e3) return "";
  const rate = (s[s.length - 1][1] - s[0][1]) / ((s[s.length - 1][0] - s[0][0]) / 1000);  // % per second
  if (!(rate > 0)) return "";
  const left = (100 - p) / rate;
  return left < 90 ? " · about a minute left" : ` · about ${Math.round(left / 60)} min left`;
}
function progressBar(d, label) {
  const p = Math.max(0, Math.min(100, d.progress || 0)), size = d.size || storeModel(d.id)?.size;
  const done = typeof size === "number" ? `${(size * p / 100 / 1e9).toFixed(size < 1e9 ? 2 : 1)} of ${GB(size)}` : `${Math.floor(p)}%`;
  return el("div", { class: "load-bar mm-progress" },
    el("small", {}, `${label} · ${Math.floor(p)}% · ${done}${d.status === "downloading" ? eta(d.id, p) : ""}`),
    el("div", { class: "bar", role: "progressbar", "aria-label": `Download of ${d.name || mmName(d.id)}`, "aria-valuemin": 0, "aria-valuemax": 100,
      "aria-valuenow": Math.floor(p), "aria-valuetext": `${Math.floor(p)}%, ${done}` }, el("i", { style: `width:${p}%` })));
}
function downloadControls(d) {
  const busy = !!S.mmBusy[d.id];
  if (d.status === "error") return [el("button", { class: "btn-sm accent", disabled: busy || (nowDownloading() && nowDownloading() !== d.id), onclick: () => startDownload(d.id) }, "Try again"),
    el("button", { class: "btn-sm ghost", disabled: busy, onclick: () => cancelDownload(d.id, "Download dropped") }, "Dismiss")];
  return [d.status === "paused"
    ? el("button", { class: "btn-sm accent", disabled: busy || !!nowDownloading(), title: nowDownloading() ? "Another download is running" : "", onclick: () => startDownload(d.id) }, "Resume")
    : el("button", { class: "btn-sm", disabled: busy, onclick: () => mmPost("pause", d.id, "Paused") }, "Pause"),
    el("button", { class: "btn-sm ghost", disabled: busy, onclick: () => confirmFor("cancel", d.id, "strip") }, "Cancel")];
}
function downloadsStrip() {
  if (!S.downloads.length) return null;
  return el("div", { class: "npu mm-downloads", role: "group", "aria-label": "Downloads" },
    el("div", { class: "npu-top" }, el("span", {}, S.downloads.length === 1 ? "Download" : "Downloads"),
      S.updateQueue.length ? el("span", {}, `Updates waiting to start: ${S.updateQueue.length} `,
        el("button", { class: "btn-sm ghost", onclick: () => { S.updateQueue = []; renderDrawer(); } }, "Stop after this one")) : null),
    ...S.downloads.map((d) => el("div", { class: "mm-dl" },
      d.status === "error"
        ? el("div", { class: "hint-line warn" }, `The download of ${d.name || mmName(d.id)} failed. Does the Tiiny have internet (Wi-Fi)?`)
        : progressBar(d, `${d.name || mmName(d.id)}${d.status === "paused" ? " (paused)" : ""}`),
      S.mconfirm?.kind === "cancel" && S.mconfirm.id === d.id ? confirmBox(d.id) : el("div", { class: "row mm-actions" }, downloadControls(d)))));
}

/* ---------- Installed tab: updates waiting, missing models, each card's Manage row ---------- */
function updatesArea() {
  if (!S.store) return [el("div", { class: "hint-line mm-uptodate" }, "Checking for model updates…")];
  const waiting = S.store.models.filter((m) => m.need_update), out = [];
  for (const id of S.missing) {
    out.push(el("div", { class: "confirm mm-missing" }, el("b", {}, mmName(id)), " is missing: its old build was deleted for an update that didn't finish.",
      el("div", { class: "row" }, el("button", { class: "btn-sm ghost", onclick: () => { S.missing = S.missing.filter((x) => x !== id); saveMissing(); renderDrawer(); } }, "Dismiss"),
        el("button", { class: "btn-sm accent", disabled: !!nowDownloading(), onclick: () => startDownload(id) }, `Download again · ${GB(storeModel(id)?.size)}`))));
  }
  if (!waiting.length) {
    out.push(el("div", { class: "hint-line mm-uptodate" }, "No model updates waiting. ",
      el("span", {}, "The Tiiny can't see updates while it has no internet, so this also shows when it's offline.")));
    return out;
  }
  const total = waiting.reduce((a, m) => a + (m.size || 0), 0), free = waiting.filter((m) => !busyOnDevice(m.id) && !jobOn(m.id));
  out.push(el("div", { class: "npu mm-updates" },
    el("div", { class: "npu-top" }, el("span", {}, el("b", {}, `${waiting.length}`), ` update${waiting.length === 1 ? "" : "s"} waiting · ${GB(total)}`),
      waiting.length > 1 ? el("button", { class: "btn-sm accent", disabled: !free.length || !!nowDownloading() || updateRunning(),
        title: free.length < waiting.length ? "Loaded or busy models are skipped: unload them first" : "", onclick: () => confirmFor("updateAll", "all", "updates") }, `Update all (${free.length})`) : null),
    S.mconfirm?.kind === "updateAll" ? confirmBox("all") : null,
    ...waiting.map((m) => confirmHere("update", m.id, "updates") ? confirmBox(m.id) : el("div", { class: "other-row" },
      el("div", { class: "grow" }, el("div", {}, m.name || shortName(m.id)), el("small", {}, [GB(m.size), m.update_title].filter(Boolean).join(" · "))),
      updateButton(m, "updates")))));
  return out;
}
function blockedReason(id) {
  if (jobOn(id)) return "Busy with a job";
  if (busyOnDevice(id)) return "Unload it first";
  return "";
}
function updateButton(m, where) {
  const why = blockedReason(m.id) || (updateRunning() ? "Another update is running" : nowDownloading() ? "One download at a time" : "");
  return el("button", { class: "btn-sm", disabled: !!why || !!S.mmBusy[m.id], title: why, onclick: () => confirmFor("update", m.id, where) }, why ? `Update · ${why.toLowerCase()}` : "Update");
}
/** Is this confirm open here? A model's Update/Delete can be asked for in its card, the updates list or the Store tab. */
const confirmHere = (kinds, id, where) => !!(S.mconfirm && S.mconfirm.id === id && S.mconfirm.where === where && [].concat(kinds).includes(S.mconfirm.kind));
/** The Manage row in an installed model's card (studio.js's modelCard calls this). */
function manageRow(m) {
  const sm = storeModel(m.id);
  if (!sm) return null;
  if (confirmHere(["delete", "update"], m.id, "card")) return confirmBox(m.id);
  const facts = [GB(sm.size) + " download", sm.version != null && `build ${sm.version}`].filter(Boolean).join(" · ");
  return el("div", { class: "mcard-actions mm-manage" }, el("span", { class: "meta" }, facts),
    sm.need_update ? updateButton(sm, "card") : null,
    deleteButton(m.id, "card"));
}
function deleteButton(id, where) {
  const why = blockedReason(id);
  return el("button", { class: "btn-sm danger-btn", disabled: !!why || !!S.mmBusy[id], title: why || `Delete ${mmName(id)} from the Tiiny`,
    "aria-label": `Delete ${mmName(id)}`, onclick: () => confirmFor("delete", id, where) }, "Delete…");
}
/** Extra badge on an installed card. */
const updateBadge = (m) => (storeModel(m.id)?.need_update ? el("span", { class: "badge-cap on mm-badge" }, "Update available") : null);

/* ---------- Store tab ---------- */
const WS_NAMES = { all: "All", image: "Image", audio: "Audio", "3d": "3D", video: "Video", text: "Chat", other: "Other" };
function storeView() {
  if (!S.store) return [el("div", { class: "empty", style: "margin:40px auto" }, el("p", {}, S.storeErr ? `Can't read the Model Store: ${S.storeErr}` : "Reading the Model Store…"))];
  const q = S.storeQuery.trim().toLowerCase();
  const list = S.store.models.filter((m) => m.in_store
    && (S.storeWs === "all" || m.workspace === S.storeWs)
    && !(S.hideInstalled && m.installed)
    && (!q || [m.id, m.name, m.type, m.desc, ...(m.supported || [])].join(" ").toLowerCase().includes(q)))
    .sort((a, b) => (a.recommend_rank ?? 1e9) - (b.recommend_rank ?? 1e9));
  const search = el("input", { type: "search", class: "input mm-search", placeholder: "Search the store", "aria-label": "Search the store", value: S.storeQuery,
    oninput: (e) => { S.storeQuery = e.target.value; renderDrawer(); } });
  const ws = el("select", { class: "input", "aria-label": "Show models for", onchange: (e) => { S.storeWs = e.target.value; store.set("storeWs", S.storeWs); renderDrawer(); } },
    Object.entries(WS_NAMES).map(([k, v]) => el("option", { value: k, selected: S.storeWs === k }, v)));
  const hide = el("label", { class: "chat-opt" }, el("input", { type: "checkbox", checked: S.hideInstalled,
    onchange: (e) => { S.hideInstalled = e.target.checked; store.set("storeHideInstalled", S.hideInstalled); renderDrawer(); } }), " Hide installed");
  const disk = S.store.disk;
  return [
    S.store.store_error ? el("div", { class: "hint-line warn" }, `${S.store.store_error} Installed models can still be updated and deleted.`) : null,
    el("div", { class: "mm-filters" }, search, ws, hide),
    el("div", { class: "hint-line" }, disk ? `Storage on the Tiiny: ${Math.round(disk.free_gb)} GB free of ${Math.round(disk.total_gb)} GB. ` : "",
      "The Tiiny downloads over its own Wi-Fi, even when the studio is connected by USB."),
    el("div", { class: "section-title" }, `${list.length} model${list.length === 1 ? "" : "s"}`),
    ...(list.length ? list.map(storeCard) : [el("p", { class: "hint-line" }, "No model matches.")]),
  ];
}
function storeAction(m) {
  if (confirmHere("download", m.id, "store")) return confirmBox(m.id);
  if (UNDER_WAY.includes(m.status)) {
    const d = S.downloads.find((x) => x.id === m.id) || m;
    return m.status === "error" ? el("div", { class: "hint-line warn" }, "The download failed. See the top of this list.") : progressBar(d, m.status === "paused" ? "Paused" : "Downloading");
  }
  if (m.installed) {  // every installed model can be updated or deleted here, the "Other models" rows' too
    if (confirmHere(["delete", "update"], m.id, "store")) return confirmBox(m.id);
    return el("div", { class: "mcard-actions" }, el("span", { class: "meta" }, m.need_update ? "Installed · update available" : "Installed"),
      m.need_update ? updateButton(m, "store") : null, deleteButton(m.id, "store"));
  }
  const disk = S.store.disk, need = (m.size || 0) / 1e9 + SPACE_FLOOR_GB;
  if (disk && need > disk.free_gb)
    return el("div", { class: "hint-line warn" }, `Needs ${GB(m.size)} plus ${SPACE_FLOOR_GB} GB to spare; the Tiiny has ${disk.free_gb.toFixed(1)} GB free.`);
  const other = nowDownloading();
  return el("div", { class: "mcard-actions" }, el("span", { class: "meta" }, other ? `One download at a time: ${mmName(other)} is downloading` : ""),
    el("button", { class: "btn-sm accent", disabled: !!other || !!S.mmBusy[m.id], "aria-label": `Download · ${GB(m.size)}, ${m.name || shortName(m.id)}`,
      onclick: () => confirmFor("download", m.id, "store") }, `Download · ${GB(m.size)}`));
}
const when = (t) => (typeof t === "number" && t > 0 ? new Date(t * 1000).toLocaleDateString() : null);
function factsList(m) {
  const th = m.thinking && typeof m.thinking === "object" ? m.thinking : null;
  const rows = [
    ["Model id", m.id], ["Type", m.type], ["Parameters", m.params], ["NPU memory", m.npu_usage != null && `${m.npu_usage}%`],
    ["Download", m.size && GB(m.size)], ["Context", m.context_window && `${Number(m.context_window).toLocaleString()} tokens`],
    ["Loads in", m.load_time && `about ${fmtSec(m.load_time)}`], ["Takes", m.input], ["Gives", m.output],
    ["Features", (m.supported || []).join(", ")], ["Capabilities", (m.capabilities || []).join(", ")],
    ["Thinking", th && (th.supported ? (th.toggleable ? "yes, can be switched off" : "always on") : "no")],
    ["Reasoning levels", (m.reasoning_levels || []).join(", ")], ["Image sizes", m.image_size && String(m.image_size).replace(/\(recommended\)/g, " (recommended)").split(";").join(", ")],
    ["Steps", m.steps], ["Guidance", m.guidance_scale], ["Downloads", m.downloads != null && Number(m.downloads).toLocaleString()], ["Likes", m.likes],
    ["Added to the store", when(m.created_time)], ["Updated in the store", when(m.updated_time)],
    ["Store label", m.version_label && `${m.version_label} (the store's own label; the build a download installs can differ)`],
    ["Installed build", m.installed && m.version], ["Update notes", m.need_update && [m.update_title, m.update_desc].filter(Boolean).join(": ")],
  ].filter(([, v]) => v != null && v !== false && v !== "");
  return el("dl", { class: "mm-facts" }, rows.map(([k, v]) => [el("dt", {}, k), el("dd", {}, String(v))]));
}
function storeCard(m) {
  const status = m.installed ? "running" : UNDER_WAY.includes(m.status) ? "loading" : "";
  const statusText = m.installed ? "installed" : m.status === "not_downloaded" ? "" : m.status;
  const card = el("div", { class: "mcard mm-store" },
    el("div", { class: "mcard-top" }, el("div", { class: "ico" }, modelImg(m)),
      el("div", { class: "info" },
        el("div", { class: "nm" }, m.name || shortName(m.id), statusText ? el("span", { class: "status " + status }, statusText) : null),
        el("div", { class: "sub" }, [m.params, m.type, m.size && GB(m.size), m.npu_usage != null && `${m.npu_usage}% NPU`].filter(Boolean).join(" · ")))),
    (m.supported || []).length ? el("div", { class: "badges" }, m.supported.map((b) => el("span", { class: "badge-cap on" }, b))) : null,
    storeAction(m));
  const d = el("details", { class: "desc", open: !!S.openDesc?.["store:" + m.id] }, el("summary", {}, "Details"),
    m.desc ? el("p", {}, String(m.desc).replace(/\*\*/g, "")) : null, factsList(m));
  d.addEventListener("toggle", () => { (S.openDesc ||= {})["store:" + m.id] = d.open; });
  card.append(d);
  return card;
}

/* ---------- confirms (inline, focus on Cancel) and actions ---------- */
function confirmFor(kind, id, where = "card") {
  S.mconfirm = { kind, id, where }; renderDrawer();
  $("#drawer .mm-confirm [data-cancel]")?.focus();
}
function closeConfirm() { S.mconfirm = null; renderDrawer(); }
function confirmBox(id) {
  const kind = S.mconfirm.kind, m = storeModel(id) || {}, name = m.name || mmName(id), size = GB(m.size);
  const waiting = (S.store?.models || []).filter((x) => x.need_update && !busyOnDevice(x.id) && !jobOn(x.id));
  const text = {
    download: [`Download `, el("b", {}, name), ` (${size})? The Tiiny downloads it over its own Wi-Fi, even when the studio is connected by USB. On a phone hotspot that is ${size} of your data.`],
    delete: [`Delete `, el("b", {}, name), ` (${size}) from your Tiiny? To use it again you'll have to download it again (${size} over the Tiiny's Wi-Fi).`],
    update: [`Update `, el("b", {}, name), `? The studio deletes the installed build, then downloads the new one (${size} over the Tiiny's Wi-Fi). If the download fails, ${name} is missing until you download it again.`,
      m.update_title || m.update_desc ? el("div", { class: "mm-notes" }, [m.update_title, m.update_desc].filter(Boolean).join(": ")) : null],
    updateAll: [`Update ${waiting.length} model${waiting.length === 1 ? "" : "s"}, ${GB(waiting.reduce((a, x) => a + (x.size || 0), 0))} of downloads, one after another? Each is deleted first, so a failed download leaves it missing until you download it again. Keep the studio open until they're done.`],
    cancel: S.updateIds.has(id)
      ? [`Stop downloading `, el("b", {}, name), `? Its old build was already deleted for this update, so ${name} stays missing until you download it again.`]
      : [`Stop downloading `, el("b", {}, name), `? What's downloaded so far is deleted; downloading it later starts from the beginning.`],
  }[kind];
  const go = { download: ["Download", () => startDownload(id)], delete: [`Delete ${name}`, () => deleteModel(id)], update: ["Delete and update", () => updateModel(id)],
    updateAll: ["Update all", () => updateAll(waiting.map((x) => x.id))], cancel: ["Stop the download", () => cancelDownload(id, "Download stopped")] }[kind];
  // a second guard behind the disabled buttons (the servers refuse these too)
  const blocked = ["delete", "update"].includes(kind) ? blockedReason(id) || (kind === "update" && updateRunning() ? "Another update is running" : "")
    : ["download", "updateAll"].includes(kind) && nowDownloading() ? "One download at a time" : kind === "updateAll" && updateRunning() ? "Another update is running" : "";
  const box = el("div", { class: "confirm mm-confirm", role: "group", "aria-label": go[0] }, ...text,
    blocked ? el("div", { class: "hint-line warn" }, `${blocked}.`) : null,
    el("div", { class: "row" },
      el("button", { class: "btn-sm ghost", "data-cancel": "", onclick: closeConfirm }, "Cancel"),
      el("button", { class: "btn-sm " + (kind === "delete" || kind === "update" || kind === "updateAll" ? "danger-btn" : "accent"), disabled: !!blocked, onclick: () => { S.mconfirm = null; go[1](); } }, go[0])));
  box.addEventListener("keydown", (e) => { if (e.key === "Escape") { e.stopPropagation(); closeConfirm(); } });
  return box;
}
async function mmPost(action, id, done, body = {}) {
  S.mmBusy[id] = true; renderDrawer();
  try {
    await api("/api/model/" + action, { method: "POST", body: JSON.stringify({ id, ...body }) });
    if (done) toast(`${done}: ${mmName(id)}`);
    return true;
  } catch (e) {
    if (e.type === "update_missing" && !S.missing.includes(id)) { S.missing.push(id); saveMissing(); }
    toast(e.message, "err");
    return false;
  } finally {
    delete S.mmBusy[id];
    await loadStore();
    loadModels(true);
  }
}
async function startDownload(id) { await mmPost("download", id, "Downloading"); }
async function deleteModel(id) { await mmPost("delete", id, "Deleted", { confirm: id }); }
async function cancelDownload(id, done) {
  S.cancelled.add(id);  // when it leaves the list, it wasn't a finished download
  const wasUpdate = S.updateIds.has(id);
  if (wasUpdate) stopQueue();  // first: the poll that sees it gone must not start the next update
  if (!(await mmPost("cancel", id, done))) { S.cancelled.delete(id); return; }
  if (wasUpdate && !S.missing.includes(id)) { S.missing.push(id); saveMissing(); }  // its old build is gone
  if (S.updating?.id === id) S.updating = null;
  renderDrawer();
}
async function updateModel(id) {
  S.updateInFlight = true;  // no other update starts while this request runs (the servers would refuse it anyway)
  const ok = await mmPost("update", id, "Updating", { confirm: id });
  S.updateInFlight = false;
  if (ok) { S.updating = { id, since: Date.now() }; S.updateIds.add(id); }
  return ok;
}
/** Update all: one at a time from here; each is a single safe request, so closing the studio just stops the next. */
async function updateAll(ids) {
  S.updateQueue = ids.slice();
  await nextUpdate();
}
async function nextUpdate() {
  if (!S.updateQueue.length || nowDownloading() || S.updating || S.updateInFlight) return;
  const id = S.updateQueue.shift();
  if (!(await updateModel(id))) stopQueue();
  renderDrawer();
}
