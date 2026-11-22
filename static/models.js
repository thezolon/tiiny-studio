"use strict";
/* ---------- model store: browse, download, the download queue, pause, cancel, updates, delete ----------
   Two tabs in the Models drawer: Installed (studio.js draws the cards; this adds the updates area and each card's
   Manage row) and Store. Requested on the Tiiny Discord. One download runs at a time; more wait in the download queue,
   which lives in the server (it keeps going with this window closed) and can lose one item or all of them. The servers
   refuse anything unsafe (a loaded or busy model, a second download, too little space); this file explains it before
   the user gets there. Text from the device (descriptions, update notes) only ever goes in as text. */

const GB = (b) => (typeof b === "number" && b > 0 ? `${(b / 1e9).toFixed(b < 1e9 ? 2 : 1)} GB` : "size unknown");
const SPACE_FLOOR_GB = 2;  // the servers keep this much free after a download
const UNDER_WAY = ["downloading", "paused", "error"];
Object.assign(S, {
  mtab: store.get("modelsTab", "installed"), store: null, storeErr: null, downloads: [], storeQuery: "", storeWs: store.get("storeWs", "all"),
  hideInstalled: store.get("storeHideInstalled", false), mconfirm: null, dlSamples: {}, dlSeen: {}, mmBusy: {},
  dlQueue: { items: [], stopped: null },  // the server's download queue (S.queue is the image job queue)
  missing: [],                          // models an update deleted and couldn't put back (the server keeps the list)
  checking: new Set(),                  // downloads that left the list: finished, cancelled or failed? the store says which
  cancelled: new Set(),                 // cancelled from here: no "finished" toast when they disappear
  pollSeq: 0, pollShown: 0,
});

const storeModel = (id) => (S.store?.models || []).find((m) => m.id === id);
const mmName = (id) => storeModel(id)?.name || shortName(id);
const busyOnDevice = (id) => { const m = modelById(id); return !!(m && (m.status === "running" || m.status === "loading" || S.loading[id])); };
const jobOn = (id) => (S.jobs || []).some((j) => j.model === id && (j.status === "queued" || j.status === "running"));
const nowDownloading = () => (S.downloads.find((d) => d.status === "downloading") || {}).id || null;  // fresh every poll
const queuePos = (id) => S.dlQueue.items.findIndex((i) => i.id === id);
/** Something is downloading, paused or waiting: a new download or update joins the queue instead of starting. */
const queueBusy = () => S.downloads.some((d) => d.status !== "error") || S.dlQueue.items.length > 0;
const isUpdateDownload = (id) => !!(S.downloads.find((d) => d.id === id)?.update || storeModel(id)?.update);

/* ---------- polling ---------- */
// Both polls feed noteDownloads; an answer that arrives after a newer one is dropped, so nothing is announced twice.
async function loadStore() {
  const seq = ++S.pollSeq;
  let j;
  try { j = await api("/api/store"); }
  catch (e) { S.storeErr = e.message; renderDrawer(); return; }  // the catalog's own poll explains an offline or locked Tiiny
  if (seq < S.pollShown) return;
  S.pollShown = seq; S.store = j; S.storeErr = null;
  noteDownloads(j.models.filter((m) => UNDER_WAY.includes(m.status)).map((m) => ({ id: m.id, name: m.name, status: m.status, progress: m.progress, size: m.size, update: m.update })),
    j, j.models);
  renderDrawer();
}
async function loadDownloads() {
  const seq = ++S.pollSeq;
  let j;
  try { j = await api("/api/downloads"); } catch { return; }
  if (seq < S.pollShown) return;
  S.pollShown = seq;
  noteDownloads(j.downloads || [], j);
}
/** Compare with the last poll. A download that left the list is checked against the store before anything is said. */
function noteDownloads(list, answer, models) {
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
  const stoppedBefore = S.dlQueue.stopped;
  S.downloads = list; S.dlSeen = now;
  if (answer.queue) S.dlQueue = answer.queue;
  if (answer.missing) S.missing = answer.missing;
  if (S.dlQueue.stopped && S.dlQueue.stopped !== stoppedBefore) toast(`The download queue stopped. ${S.dlQueue.stopped}`, "err");
  if (S.checking.size) { if (models) settle(models, now); else loadStore(); }
  renderModelBtn();
}
function settle(models, now) {
  const done = [];
  for (const id of [...S.checking]) {
    S.checking.delete(id);
    if (now[id]) continue;  // back in the list: still under way
    delete S.dlSamples[id];
    if (models.find((x) => x.id === id)?.installed) { done.push(id); continue; }
    if (S.cancelled.delete(id)) continue;  // the user stopped it: they know
    toast(`${mmName(id)} stopped downloading and isn't on your Tiiny.`, "err");
  }
  if (done.length) {
    for (const id of done) announce(`${mmName(id)}: download finished`);
    toast(done.length === 1 ? `${mmName(done[0])} is on your Tiiny` : `${done.length} downloads finished`);
    loadModels(true);
  }
}
// Fast while the drawer is open (the catalog poll in studio.js runs then too), every 5 s while something is going on.
let lastStorePoll = 0;
setInterval(() => {
  const open = S.drawer, active = S.downloads.length || S.dlQueue.items.length || S.checking.size;
  const every = open ? 2500 : active ? 5000 : 0;
  if (!every || Date.now() - lastStorePoll < every || S.deviceOk === false) return;
  lastStorePoll = Date.now();
  (open || S.checking.size ? loadStore : loadDownloads)();
}, 1000);
loadDownloads();  // a download started earlier (or in the TiinyOS app) shows from the start

/* ---------- the header button: "Downloading X · 34%" while a download runs ---------- */
function dlBadge() {
  const d = S.downloads.find((x) => x.status === "downloading");
  if (!d || S.deviceOk === false) return;
  const more = S.dlQueue.items.length;
  $("#modelCaps").textContent = `Downloading ${d.name || mmName(d.id)} · ${Math.floor(d.progress || 0)}%${more ? ` · ${more} queued` : ""}`;
}

/* ---------- drawer: tabs, downloads and the queue ---------- */
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
  // the GB part only when the size is known (a model that isn't in the store has none): no "41% · 41%"
  const done = typeof size === "number" ? `${(size * p / 100 / 1e9).toFixed(size < 1e9 ? 2 : 1)} of ${GB(size)}` : "";
  return el("div", { class: "load-bar mm-progress" },
    el("small", {}, `${label} · ${Math.floor(p)}%${done ? ` · ${done}` : ""}${d.status === "downloading" ? eta(d.id, p) : ""}`),
    el("div", { class: "bar", role: "progressbar", "aria-label": `Download of ${d.name || mmName(d.id)}`, "aria-valuemin": 0, "aria-valuemax": 100,
      "aria-valuenow": Math.floor(p), "aria-valuetext": done ? `${Math.floor(p)}%, ${done}` : `${Math.floor(p)}%` }, el("i", { style: `width:${p}%` })));
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
function queueList() {
  const q = S.dlQueue, items = q.items;
  if (!items.length && !q.stopped) return null;
  const total = items.reduce((a, i) => a + (i.size || 0), 0);
  return el("div", { class: "mm-queue" },
    el("div", { class: "npu-top" }, el("span", {}, el("b", {}, `Queue · ${items.length}`), items.length ? ` · ${GB(total)}, one after another` : ""),
      items.length ? el("button", { class: "btn-sm ghost", onclick: () => queuePost("dequeue", { all: true }, "Queue cleared") }, "Clear queue") : null),
    items.length && S.downloads.some((d) => d.status === "paused") ? el("div", { class: "hint-line" }, "A download is paused, so the queue is waiting. Resume it or cancel it and the next one starts.") : null,
    q.stopped ? el("div", { class: "confirm mm-stopped" }, el("b", {}, "The queue stopped. "), q.stopped,
      el("div", { class: "row" }, el("button", { class: "btn-sm accent", onclick: () => queuePost("resume", {}, "Queue resumed") }, "Resume the queue"))) : null,
    items.length ? el("ol", { class: "mm-queue-list", "aria-label": "Waiting to download" }, items.map((i) => el("li", { class: "other-row" },
      el("div", { class: "grow" }, el("div", {}, i.name || shortName(i.id)), el("small", {}, [GB(i.size), i.kind === "update" && "update: the installed build is deleted first"].filter(Boolean).join(" · "))),
      el("button", { class: "btn-sm ghost", "aria-label": `Remove ${i.name || shortName(i.id)} from the queue`, onclick: () => queuePost("dequeue", { id: i.id }, `Removed from the queue: ${i.name || shortName(i.id)}`) }, "Remove")))) : null);
}
function downloadsStrip() {
  const queue = queueList();
  if (!S.downloads.length && !queue) return null;
  return el("div", { class: "npu mm-downloads", role: "group", "aria-label": "Downloads" },
    S.downloads.length ? el("div", { class: "npu-top" }, el("span", {}, S.downloads.length === 1 ? "Download" : "Downloads")) : null,
    ...S.downloads.map((d) => el("div", { class: "mm-dl" },
      d.status === "error"
        ? el("div", { class: "hint-line warn" }, `The download of ${d.name || mmName(d.id)} failed. Does the Tiiny have internet (Wi-Fi)?`)
        : progressBar(d, `${d.name || mmName(d.id)}${d.update ? " (update)" : ""}${d.status === "paused" ? " (paused)" : ""}`),
      confirmHere("cancel", d.id, "strip") ? confirmBox(d.id) : el("div", { class: "row mm-actions" }, downloadControls(d)))),
    queue);
}

/* ---------- Installed tab: updates waiting, missing models, each card's Manage row ---------- */
function updatesArea() {
  if (!S.store) return [el("div", { class: "hint-line mm-uptodate" }, "Checking for model updates…")];
  const waiting = S.store.models.filter((m) => m.need_update), out = [];
  for (const id of S.missing) {
    out.push(el("div", { class: "confirm mm-missing" }, el("b", {}, mmName(id)), " is missing: its old build was deleted for an update that didn't finish.",
      el("div", { class: "row" }, el("button", { class: "btn-sm ghost", onclick: () => queuePost("dismiss", { id }) }, "Dismiss"),
        el("button", { class: "btn-sm accent", onclick: () => getModel(id) }, `${queueBusy() ? "Queue the download" : "Download again"} · ${GB(storeModel(id)?.size)}`))));
  }
  if (!waiting.length) {
    out.push(el("div", { class: "hint-line mm-uptodate" }, "No updates found. ",
      el("span", {}, "Updates only show while the Tiiny is online.")));
    return out;
  }
  const total = waiting.reduce((a, m) => a + (m.size || 0), 0);
  const free = waiting.filter((m) => !busyOnDevice(m.id) && !jobOn(m.id) && queuePos(m.id) < 0);
  out.push(el("div", { class: "npu mm-updates" },
    el("div", { class: "npu-top" }, el("span", {}, el("b", {}, `${waiting.length}`), ` update${waiting.length === 1 ? "" : "s"} waiting · ${GB(total)}`),
      waiting.length > 1 ? el("button", { class: "btn-sm accent", disabled: !free.length,
        title: free.length < waiting.length ? "Loaded, busy or already queued models are left out: unload them first" : "",
        onclick: () => confirmFor("updateAll", "all", "updates") }, `Update all (${free.length})`) : null),
    confirmHere("updateAll", "all", "updates") ? confirmBox("all") : null,
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
  if (queuePos(m.id) >= 0) return el("span", { class: "meta" }, `Queued · #${queuePos(m.id) + 1}`);
  const why = blockedReason(m.id);
  const label = queueBusy() ? "Queue update" : "Update";
  return el("button", { class: "btn-sm", disabled: !!why || !!S.mmBusy[m.id], title: why, onclick: () => confirmFor("update", m.id, where) }, why ? `Update · ${why.toLowerCase()}` : label);
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
  const why = blockedReason(id) || (queuePos(id) >= 0 ? "Queued for an update" : "");
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
      "The Tiiny downloads over its own Wi-Fi, even when the studio is connected by USB. Downloads run one at a time; more wait in the queue."),
    el("div", { class: "section-title" }, `${list.length} model${list.length === 1 ? "" : "s"}`),
    ...(list.length ? list.map(storeCard) : [el("p", { class: "hint-line" }, "No model matches.")]),
  ];
}
function storeAction(m) {
  if (confirmHere(["download", "queue"], m.id, "store")) return confirmBox(m.id);
  if (UNDER_WAY.includes(m.status)) {
    const d = S.downloads.find((x) => x.id === m.id) || m;
    return m.status === "error" ? el("div", { class: "hint-line warn" }, "The download failed. See the top of this list.") : progressBar(d, m.status === "paused" ? "Paused" : "Downloading");
  }
  if (m.installed) {  // every installed model can be updated or deleted here, the "Other models" rows' too
    if (confirmHere(["delete", "update"], m.id, "store")) return confirmBox(m.id);
    return el("div", { class: "mcard-actions" }, el("span", { class: "meta" }, m.need_update ? "Installed · update available" : "Installed"),
      m.need_update ? updateButton(m, "store") : null, deleteButton(m.id, "store"));
  }
  const pos = queuePos(m.id);
  if (pos >= 0) return el("div", { class: "mcard-actions" }, el("span", { class: "meta" }, `Queued · #${pos + 1}`),
    el("button", { class: "btn-sm ghost", "aria-label": `Remove ${m.name || shortName(m.id)} from the queue`,
      onclick: () => queuePost("dequeue", { id: m.id }, `Removed from the queue: ${m.name || shortName(m.id)}`) }, "Remove from queue"));
  const disk = S.store.disk, need = (m.size || 0) / 1e9 + SPACE_FLOOR_GB;
  if (disk && need > disk.free_gb)
    return el("div", { class: "hint-line warn" }, `Needs ${GB(m.size)} plus ${SPACE_FLOOR_GB} GB to spare; the Tiiny has ${disk.free_gb.toFixed(1)} GB free.`);
  const queued = queueBusy(), label = `${queued ? "Add to queue" : "Download"} · ${GB(m.size)}`;
  return el("div", { class: "mcard-actions" }, el("span", { class: "meta" }, queued ? "Starts when the downloads before it are done" : ""),
    el("button", { class: "btn-sm accent", disabled: !!S.mmBusy[m.id], "aria-label": `${label}, ${m.name || shortName(m.id)}`,
      onclick: () => confirmFor(queued ? "queue" : "download", m.id, "store") }, label));
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
  return el("dl", { class: "mm-facts" }, rows.flatMap(([k, v]) => [el("dt", {}, k), el("dd", {}, String(v))]));
}
function storeCard(m) {
  const status = m.installed ? "running" : UNDER_WAY.includes(m.status) || queuePos(m.id) >= 0 ? "loading" : "";
  const statusText = m.installed ? "installed" : m.status !== "not_downloaded" ? m.status : queuePos(m.id) >= 0 ? "queued" : "";
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
  const waiting = (S.store?.models || []).filter((x) => x.need_update && !busyOnDevice(x.id) && !jobOn(x.id) && queuePos(x.id) < 0);
  const wifi = `The Tiiny downloads over its own Wi-Fi, even when the studio is connected by USB. On a phone hotspot that is ${size} of your data.`;
  const updateText = `The studio deletes the installed build, then downloads the new one (${size} over the Tiiny's Wi-Fi). If the download fails, ${name} is missing until you download it again.`;
  const notes = m.update_title || m.update_desc ? el("div", { class: "mm-notes" }, [m.update_title, m.update_desc].filter(Boolean).join(": ")) : null;
  const queued = queueBusy();
  const text = {
    download: [`Download `, el("b", {}, name), ` (${size})? ${wifi}`],
    queue: [`Add `, el("b", {}, name), ` (${size}) to the download queue? It starts when the downloads before it are done, even with this window closed. ${wifi}`],
    delete: [`Delete `, el("b", {}, name), ` (${size}) from your Tiiny? To use it again you'll have to download it again (${size} over the Tiiny's Wi-Fi).`],
    update: [queued ? `Add the update of ` : `Update `, el("b", {}, name), queued ? ` to the download queue? When its turn comes: ${updateText.charAt(0).toLowerCase()}${updateText.slice(1)}` : `? ${updateText}`, notes],
    updateAll: [`Update ${waiting.length} model${waiting.length === 1 ? "" : "s"}, ${GB(waiting.reduce((a, x) => a + (x.size || 0), 0))} of downloads, one after another in the download queue? Each is deleted first, so a failed download leaves it missing until you download it again. If one fails, the queue stops there.`],
    cancel: isUpdateDownload(id)
      ? [`Stop downloading `, el("b", {}, name), `? Its old build was already deleted for this update, so ${name} stays missing until you download it again.`]
      : [`Stop downloading `, el("b", {}, name), `? What's downloaded so far is deleted; downloading it later starts from the beginning.`],
  }[kind];
  const go = {
    download: ["Download", () => startDownload(id)],
    queue: ["Add to queue", () => queuePost("queue", { ids: [id], kind: "download" }, `Queued: ${name}`)],
    delete: [`Delete ${name}`, () => deleteModel(id)],
    update: [queued ? "Queue the update" : "Delete and update", () => (queued ? queuePost("queue", { ids: [id], kind: "update", confirm: [id] }, `Update queued: ${name}`) : updateModel(id))],
    updateAll: ["Update all", () => { const ids = waiting.map((x) => x.id); return queuePost("queue", { ids, kind: "update", confirm: ids }, `${ids.length} update${ids.length === 1 ? "" : "s"} queued`); }],
    cancel: ["Stop the download", () => cancelDownload(id, "Download stopped")],
  }[kind];
  // a second guard behind the disabled buttons (the servers refuse these too)
  const blocked = ["delete", "update"].includes(kind) ? blockedReason(id) : "";
  const box = el("div", { class: "confirm mm-confirm", role: "group", "aria-label": go[0] }, ...text,
    blocked ? el("div", { class: "hint-line warn" }, `${blocked}.`) : null,
    el("div", { class: "row" },
      el("button", { class: "btn-sm ghost", "data-cancel": "", onclick: closeConfirm }, "Cancel"),
      el("button", { class: "btn-sm " + (["delete", "update", "updateAll"].includes(kind) ? "danger-btn" : "accent"), disabled: !!blocked, onclick: () => { S.mconfirm = null; go[1](); } }, go[0])));
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
    toast(e.message, "err");
    return false;
  } finally {
    delete S.mmBusy[id];
    await loadStore();
    loadModels(true);
  }
}
/** queue {ids, kind, confirm}, dequeue {id} or {all}, resume, dismiss {id}: the server's download queue. */
async function queuePost(action, body, done) {
  try {
    await api("/api/model/" + action, { method: "POST", body: JSON.stringify(body) });
    if (done) toast(done);
  } catch (e) { toast(e.message, "err"); }
  await loadStore();
}
async function startDownload(id) { await mmPost("download", id, "Downloading"); }
/** Download now, or join the queue when something else is under way (the "missing" note uses this). */
function getModel(id) {
  return queueBusy() ? queuePost("queue", { ids: [id], kind: "download" }, `Queued: ${mmName(id)}`) : startDownload(id);
}
async function deleteModel(id) { await mmPost("delete", id, "Deleted", { confirm: id }); }
async function updateModel(id) { await mmPost("update", id, "Updating", { confirm: id }); }
async function cancelDownload(id, done) {
  S.cancelled.add(id);  // when it leaves the list, it wasn't a finished download
  if (!(await mmPost("cancel", id, done))) S.cancelled.delete(id);
}
