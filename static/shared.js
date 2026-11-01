"use strict";
/* Shared plumbing for the Audio and 3D workspaces: workspace switching, server-side jobs,
   model gating, and audio helpers (decode, WAV encode, record, waveform player). */

/* ---------- workspaces ---------- */
S.ws = store.get("ws", "image");
// audio.js / mesh.js load after this file; a fetch can finish before they run, so look them up lazily.
const WS = { image: { el: "#wsImage" }, audio: { el: "#wsAudio", mod: () => (typeof AudioWS === "undefined" ? null : AudioWS) },
  mesh: { el: "#wsMesh", mod: () => (typeof MeshWS === "undefined" ? null : MeshWS) },
  video: { el: "#wsVideo", mod: () => (typeof VideoWS === "undefined" ? null : VideoWS) },
  chat: { el: "#wsChat", mod: () => (typeof ChatWS === "undefined" ? null : ChatWS) } };
function setWs(ws) {
  if (!WS[ws]) ws = "image";
  S.ws = ws; store.set("ws", ws);
  for (const [k, v] of Object.entries(WS)) $(v.el).hidden = k !== ws;
  for (const b of document.querySelectorAll(".ws-tabs [data-ws]")) { b.setAttribute("aria-selected", b.dataset.ws === ws); b.tabIndex = b.dataset.ws === ws ? 0 : -1; }  // roving tabindex
  $("#dropveilText").textContent = { image: "Drop to use as source image", audio: "Drop audio to use it here", mesh: "Drop an image to turn into 3D", video: "Drop a picture to start the clip from", chat: "Drop pictures or documents to attach" }[ws];
  const mod = WS[ws].mod?.();
  if (mod) { if (!mod.mounted) { mod.mount($(WS[ws].el)); mod.mounted = true; } mod.render(); }
  renderModelBtn(); renderDrawer();
}
for (const b of document.querySelectorAll(".ws-tabs [data-ws]")) b.onclick = () => setWs(b.dataset.ws);
// WAI-ARIA tabs: arrows move between workspaces (and open them), Home/End jump to the ends
$(".ws-tabs").addEventListener("keydown", (e) => {
  const tabs = [...document.querySelectorAll(".ws-tabs [data-ws]")], i = tabs.indexOf(e.target);
  const to = { ArrowRight: (i + 1) % tabs.length, ArrowLeft: (i + tabs.length - 1) % tabs.length, Home: 0, End: tabs.length - 1 }[e.key];
  if (i < 0 || to == null || e.ctrlKey || e.metaKey || e.altKey) return;
  e.preventDefault(); e.stopPropagation();  // the image shortcuts on document also listen to the arrows
  tabs[to].focus(); setWs(tabs[to].dataset.ws);
});
function wsFiles(files) { const mod = WS[S.ws].mod?.(); return mod?.files ? mod.files(files) : false; }
function onModelsChanged() { for (const k of ["audio", "mesh", "video"]) { const m = WS[k].mod(); if (m?.mounted && S.ws === k) m.renderGate?.(); } WS.chat.mod()?.onModels?.(); renderAssistGates(); }
function onHistoryLoaded() { for (const k of ["audio", "mesh", "video"]) { const m = WS[k].mod(); if (m?.mounted) m.render(); } }

/* ---------- server-side jobs ---------- */
S.jobs = [];
const seenDone = new Set();
const PAGE_T0 = Date.now();
let jobTimer = null;
async function submitJob(kind, payload, label) {
  const m = modelForTool(kind);
  const est = typeof payload.est === "number" ? payload.est : undefined;
  const job = await api("/api/jobs", { method: "POST", body: JSON.stringify({ kind, model: m?.id, label, est, ...payload }) });
  S.jobs.push(job); renderJobsPill(); pollJobs();
  return job;
}
async function pollJobs() {
  clearTimeout(jobTimer);
  try { S.jobs = await api("/api/jobs"); } catch { /* keep the last view */ }
  for (const j of S.jobs) {
    if (seenDone.has(j.id)) continue;
    // a job that had already finished before this page loaded is old news
    if (j.status !== "queued" && j.status !== "running" && (j.finished || j.created) * 1000 < PAGE_T0 - 3000) { seenDone.add(j.id); continue; }
    if (j.status === "done" && j.entry) {
      seenDone.add(j.id);
      if (j.kind !== "assist" && j.kind !== "chat" && !j.params?.ephemeral && !S.history.some((e) => e.id === j.entry.id)) S.history.unshift(j.entry);
      onJobDone(j);
    } else if (j.status === "error" || j.status === "cancelled") {
      seenDone.add(j.id);
      if (j.status === "error") toast(j.error || "Job failed", "err");
      onJobDone(j);
    }
  }
  // after finished results are in history, so a render never shows "nothing selected"
  announceJobs();
  if (typeof syncImageJobs === "function") syncImageJobs();
  renderJobsPill();
  for (const k of ["audio", "mesh", "video"]) { const m = WS[k].mod(); if (m?.mounted && S.ws === k) m.renderJobs?.(); }
  const active = S.jobs.some((j) => j.status === "queued" || j.status === "running");
  if (active && typeof pollStats === "function" && !statsTimer) pollStats();
  jobTimer = setTimeout(pollJobs, active ? 1500 : 15000);
}
const jobNote = new Map();  // job id -> the state last announced
/** Tell screen readers when a job queues (with its place in line), starts, or moves up the line. Done and failed come
 *  from onJobDone and the error toast; never the progress ticks. Jobs from before this page loaded stay quiet. */
function announceJobs() {
  for (const j of S.jobs) {
    if (["chat", "assist", "ocr"].includes(j.kind) || (j.status !== "queued" && j.status !== "running")) continue;
    const key = j.status === "queued" ? `queued ${j.queue_pos || 1}` : "running", was = jobNote.get(j.id), pos = j.queue_pos || 1;
    if (was === key) continue;
    jobNote.set(j.id, key);
    if (j.created * 1000 < PAGE_T0 - 3000) continue;
    const name = j.label || j.kind;
    announce(j.status === "running" ? `${name} started` : was ? `${name}: ${pos} in line` : `${name} queued${pos > 1 ? `, ${pos} in line` : ""}`);
  }
}
function onJobDone(j) {
  if (j.kind === "ocr" && typeof showOcrResult === "function") return showOcrResult(j);
  if (j.kind === "assist") { const w = ASSIST_WAIT.get(j.id); ASSIST_WAIT.delete(j.id); w?.(j.status === "done" ? j.entry.text : null); return; }
  const mod = { image: "image", speech: "audio", transcribe: "audio", loop: "audio", song: "audio", mesh: "mesh", video: "video" }[j.kind];
  if (j.kind === "image" && typeof imageJobDone === "function") imageJobDone(j);
  const m = WS[mod]?.mod?.();
  if (m?.mounted) m.jobDone?.(j);
  if (j.status === "done" && mod && S.ws !== mod) toast(`${j.label || j.kind} is ready — open ${{ image: "Image", mesh: "3D", audio: "Audio", video: "Video" }[mod]} to see it`);
  else if (j.status !== "error" && j.kind !== "chat") announce(`${j.label || j.kind} ${j.status === "done" ? "is done" : "was cancelled"}`);  // the toast above says it otherwise; errors have their own
  if (j.status === "done" && document.hidden && "Notification" in window && Notification.permission === "granted")
    new Notification("Tiiny Studio", { body: `${j.label || j.kind} is ready` });
}
function renderJobsPill() {
  const act = S.jobs.filter((j) => j.status === "queued" || j.status === "running");
  const pill = $("#jobsPill");
  pill.hidden = !act.length;
  if (!act.length) return;
  const j = act[0];
  const npu = typeof S.stats?.npu_util === "number" ? ` · NPU ${Math.round(S.stats.npu_util)}%` : "";
  pill.replaceChildren(el("span", { class: "dot busy" }), `${act.length} job${act.length > 1 ? "s" : ""} · ${j.label || j.kind}${j.progress ? ` ${j.progress}%` : ""}${npu}`);
  pill.onclick = (e) => { e.stopPropagation(); toggleJobsMenu(e.detail === 0); };
  if (!$("#jobsMenu")?.hidden) renderJobsMenu();
}
const wsOfJob = (j) => ({ mesh: "mesh", video: "video", image: "image", chat: "chat", assist: S.ws }[j.kind] || "audio");
/** Every queued and running job, from any workspace, each with Cancel. */
function renderJobsMenu() {
  let menu = $("#jobsMenu");
  if (!menu) {
    menu = el("div", { id: "jobsMenu", class: "jobs-menu", hidden: true, onclick: (e) => e.stopPropagation() });
    document.body.append(menu);
    document.addEventListener("click", () => { menu.hidden = true; });
  }
  const act = S.jobs.filter((j) => j.status === "queued" || j.status === "running");
  if (!act.length) { menu.hidden = true; return; }
  const r = $("#jobsPill").getBoundingClientRect();
  menu.style.top = `${r.bottom + 6}px`;
  requestAnimationFrame(() => { menu.style.left = `${Math.max(8, Math.min(r.left, innerWidth - menu.offsetWidth - 8))}px`; });  // stay inside the window
  const times = queueTimes(act), waiting = act.filter((j) => j.status === "queued");
  const last = times.get(act[act.length - 1].id);
  keepFocus(menu, () => menu.replaceChildren(...[...act.map((j) => el("div", { class: "jobs-menu-row" },
    el("span", { class: "dot " + (j.status === "running" ? "busy" : "") }),
    el("button", { class: "jobs-menu-label", title: "Open its workspace", onclick: () => { menu.hidden = true; setWs(wsOfJob(j)); } },
      el("b", {}, j.label || j.kind), el("small", {}, j.stopping ? STOPPING : (j.status === "running" ? (j.tool_message || j.message || "Working…") + (j.progress ? ` · ${j.progress}%` : "") : `Waiting · #${j.queue_pos || 1} in line`)
        + (j.stopping ? "" : timeNote(j, times.get(j.id))))),
    el("button", { class: "btn-sm ghost", disabled: !!j.stopping, ...onPress((b) => stopButton(b, j.id)) }, j.stopping ? "Stopping…" : j.status === "running" ? "Cancel" : "Remove"))),
    act.length > 1 || waiting.length ? el("div", { class: "jobs-menu-foot" },
      el("span", {}, last?.end ? `Queue done around ${clock(last.end)}` : "The queue's end time isn't known yet"),
      waiting.length > 1 ? el("button", { class: "btn-sm ghost", ...onPress((b) => { b.disabled = true; b.textContent = "Removing…"; Promise.all(waiting.map((w) => api(`/api/jobs/${w.id}/cancel`, { method: "POST", body: "{}" }).catch(() => {}))).then(pollJobs); }) },
        `Remove all ${waiting.length} waiting`) : null) : null].filter(Boolean)));  // replaceChildren would print a null
}
/** When each job should start and finish, from its estimate (est, seconds) and the jobs ahead of it: a running job's
 *  remaining time, then each queued one in line. Unknown from the first job without an estimate on. */
function queueTimes(act) {
  const out = new Map(), now = Date.now() / 1000;
  let t = now;
  for (const j of act) {
    if (t == null || !(j.est > 0)) { out.set(j.id, { start: j.status === "running" ? null : t }); t = null; continue; }
    const start = j.status === "running" ? (j.started || now) : t;
    const end = Math.max(now + 5, start + j.est);
    out.set(j.id, { start, end, late: j.status === "running" && start + j.est < now });
    t = end;
  }
  return out;
}
const clock = (s) => new Date(s * 1000).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
function timeNote(j, t) {
  if (!t?.end) return t?.start && j.status === "queued" ? ` · starts ~${clock(t.start)}` : "";
  if (t.late) return " · taking longer than usual";
  return j.status === "running" ? ` · done ~${clock(t.end)}` : ` · starts ~${clock(t.start)}, done ~${clock(t.end)}`;
}
function toggleJobsMenu(byKey) {
  renderJobsMenu();
  const menu = $("#jobsMenu");
  if (menu && S.jobs.some((j) => j.status === "queued" || j.status === "running")) menu.hidden = !menu.hidden;
  if (byKey && menu && !menu.hidden) menu.querySelector("button")?.focus();  // it sits at the end of the page: bring the keyboard to it
}
document.addEventListener("keydown", (e) => {
  const menu = $("#jobsMenu");
  if (e.key === "Escape" && menu && !menu.hidden) { menu.hidden = true; if (menu.contains(document.activeElement)) $("#jobsPill").focus(); }
});
const STOPPING = "Stopping: the Tiiny finishes its current step, then the result is thrown away";
/** Cancel from a button: it shows "Stopping…" at once, since a running job ends only when the device answers. */
function stopButton(btn, id) { btn.disabled = true; btn.textContent = "Stopping…"; btn.title = STOPPING; cancelJob(id); }
/** For buttons in views that redraw while a job runs (every poll): a click needs press and release on the same
 *  element, so a redraw in between swallowed it. Act on the press; keyboard activation still comes as a click. */
const onPress = (act) => ({
  onpointerdown: (e) => { if (e.button === 0) { e.preventDefault(); act(e.currentTarget); } },
  onclick: (e) => { if (e.detail === 0) act(e.currentTarget); },
});
async function cancelJob(id) { try { await api(`/api/jobs/${id}/cancel`, { method: "POST", body: "{}" }); } catch (e) { toast(e.message, "err"); } pollJobs(); }

/* Once per page load, when the Tiiny first answers: save what it finished but the gallery never got (the studio restarted
 * mid-render, or another studio made it). The first call on a studio only takes stock, so old results are never imported. */
let recoverAsked = false;
async function recoverFromDevice() {
  if (recoverAsked) return;
  recoverAsked = true;
  try {
    const r = await api("/api/device/recover", { method: "POST", body: "{}" });
    const got = r.recovered || [];
    if (!got.length) return;
    const n = (k) => got.filter((x) => x.kind === k).length;
    const parts = [["video", "video"], ["song", "song"], ["loop", "loop"]].filter(([k]) => n(k)).map(([k, w]) => `${n(k)} ${w}${n(k) > 1 ? "s" : ""}`);
    await loadHistory();
    toast(`Saved ${parts.join(" and ")} from the Tiiny that wasn't in your gallery`);
  } catch (e) { recoverAsked = false; }  // quiet: the device may just be asleep; the next page load asks again
}

/* ---------- models per tool ---------- */
const TOOL_TYPES = { speech: "Text-to-Speech", transcribe: "ASR", loop: "Music Generation", song: "Music Generation", mesh: "Image-to-3D",
  chat: "Text Generation or Image-Text-to-Text", vision: "Image-Text-to-Text with image input" };
const CHAT_TYPES = ["Image-Text-to-Text", "Text Generation"];
const seesImages = (m) => /\bImage\b/.test(m.input || "");
/** What a loaded model reports about itself (/api/model/info, #23), fetched once per model and kept; null until it's
    loaded and has answered (onLoad re-renders the asking panel then). */
S.modelInfo = {};
function loadedInfo(m, onLoad) {
  if (!m || m.status !== "running") return null;
  const got = S.modelInfo[m.id];
  if (got === undefined) {
    S.modelInfo[m.id] = "loading";
    api("/api/model/info?id=" + encodeURIComponent(m.id)).then((r) => { S.modelInfo[m.id] = r.info || {}; }).catch(() => { S.modelInfo[m.id] = {}; }).then(onLoad);
  }
  return got && got !== "loading" && Object.keys(got).length ? got : null;
}
/** How a chat model reasons, from its own "thinking" info (TiinyOS 0.1.36+): "none", "switch" (on/off) or "always"
    (like gpt-oss: it reasons whatever it's sent). Without the info (older TiinyOS): gpt-oss always, others switch. */
function thinkingMode(m) {
  const t = m?.thinking;
  if (!t || typeof t !== "object") return /gpt-oss/i.test(m?.id || "") ? "always" : "switch";
  return !t.supported ? "none" : t.toggleable ? "switch" : "always";
}
/** Helpers run on whichever chat model is loaded; otherwise suggest the leanest stock one (one that always reasons is slow). */
function chatModel(vision) {
  const list = (S.allModels || []).filter((m) => CHAT_TYPES.includes(m.type) && (!vision || seesImages(m)));
  const rank = (m) => [m.status === "running" ? 0 : 1, /abliterated|heretic|uncensored/i.test(m.id) ? 1 : 0, thinkingMode(m) === "always" ? 1 : 0, m.npu_usage ?? 100];
  const cmp = (a, b) => { const x = rank(a), y = rank(b); for (let i = 0; i < x.length; i++) if (x[i] !== y[i]) return x[i] - y[i]; return 0; };
  return list.sort(cmp)[0] || null;
}
/** The text model helpers use: the one picked in Settings (if it can do the task), else chatModel's choice. */
function helperModel(vision) {
  const id = SET.info?.helper_model;
  return (id && (S.allModels || []).find((m) => m.id === id && CHAT_TYPES.includes(m.type) && (!vision || seesImages(m)))) || chatModel(vision);
}
/** May the studio swap models on the device for this section (Settings → Model swapping)? All on by default. */
const swapOn = (section) => { const s = SET.info?.swap; return !s || (s.on && s[section] !== false); };
const helperSection = (task) => (task === "lyrics" || task === "tags" ? "audio" : "image");
/** An Image-to-Text model that finds a page's regions but doesn't read them (PP-DocLayoutV3): not an OCR model. */
const layoutOnly = (m) => /layout analysis only|does not transcribe/i.test(m.desc || "") || /DocLayout/i.test(m.id);
/** Models the device's gateway can't run (0.1.38): they work only where the studio reaches the model's own port. */
const needsWifi = (m) => /GLM-OCR|supertonic/i.test(m?.id || "");
/** A banner when the picked model needs Wi-Fi/LAN and this connection (the USB cable) only reaches the gateway. */
function wifiNote(m) {
  if (!m || S.directAccess !== false || !needsWifi(m)) return null;
  return el("div", { class: "hint-line warn" }, `${shortName(m.id)} needs a Wi-Fi/LAN connection: over this one (USB) the Tiiny's gateway can't run it. `
    + "Switch to the Tiiny's Wi-Fi address in Settings, or pick another model.");
}
/** The installed models that can do a tool's job. */
function toolModels(tool) {
  const list = (S.allModels || []).filter((m) => m.type === TOOL_TYPES[tool]);
  const isSong = (m) => /song|lyric/i.test(m.id + " " + (m.desc || "").slice(0, 300));
  return tool === "song" ? list.filter(isSong) : tool === "loop" ? list.filter((m) => !isSong(m)) : tool === "ocr" ? list.filter((m) => !layoutOnly(m)) : list;
}
/** The model a tool uses: the one picked for it (#44) while it's loaded or loading, else a loaded one, else the
 *  picked or first installed one (its tab then offers to load it). */
function modelForTool(tool) {
  if (tool === "chat" || tool === "vision") return helperModel(tool === "vision");
  const list = toolModels(tool), picked = list.find((m) => m.id === store.get("pick." + tool, null));
  if (picked && (picked.status !== "stopped" || S.loading[picked.id] != null)) return picked;
  return list.find((m) => m.status === "running") || picked || list[0] || null;
}
/** A card naming the model in use that opens a list of the others, when several can do the job; else null.
 *  A loaded model in the list can be picked; each has its own Load or Unload button (nothing loads by itself).
 *  The list floats over the page (like the jobs menu), so the panel can redraw under it while it's open. */
function modelPicker(models, cur, choose) {
  if (models.length < 2) return null;
  const m = models.find((x) => x.id === cur) || models[0], ready = m.status === "running", loading = S.loading[m.id] != null || m.status === "loading";
  const btn = el("button", { class: "pick-btn", "aria-haspopup": "true", "aria-expanded": "false", title: "Change model",
    onclick: (e) => { e.stopPropagation(); openPicker(btn, models, m.id, choose, e.detail === 0); } },
    modelImg(m, { class: "pick-ico" }),
    el("span", { class: "pick-name" }, el("b", {}, shortName(m.id)), el("small", {}, el("i", { class: "dot " + (ready ? "ok" : loading ? "busy" : "") }),
      ready ? "Loaded" : loading ? "Loading…" : `Not loaded · ${m.npu_usage ?? "?"}% NPU`)),
    el("span", { class: "pick-change" }, ready ? `${models.length} models` : "", icon("i-chev")));
  return el("div", { class: "pick-card" }, btn, ready ? null
    : el("button", { class: "btn-sm accent", disabled: loading, "aria-label": `Load ${shortName(m.id)}`, title: `Loads in about ${fmtSec(m.load_time || 30)}`, onclick: () => loadModel(m.id) }, loading ? "Loading…" : "Load"));
}
function openPicker(btn, models, cur, choose, byKey) {
  let menu = $("#pickMenu");
  if (!menu) {
    menu = el("div", { id: "pickMenu", class: "jobs-menu pick-menu", hidden: true, onclick: (e) => e.stopPropagation(), onkeydown: (e) => {
      const rows = [...menu.querySelectorAll("button")], i = rows.indexOf(document.activeElement);
      const to = { ArrowDown: i + 1, ArrowUp: i - 1, Home: 0, End: rows.length - 1 }[e.key];
      if (to != null) { e.preventDefault(); rows[(to + rows.length) % rows.length].focus(); }
      if (e.key === "Tab") closePicker();
    } });
    document.body.append(menu);
    document.addEventListener("click", () => closePicker());
    document.addEventListener("keydown", (e) => { if (e.key === "Escape" && !menu.hidden) { e.stopPropagation(); closePicker(true); } }, true);
  }
  if (!menu.hidden && menu.anchor === btn) return closePicker();
  const row = (x) => {
    const ready = x.status === "running", loading = S.loading[x.id] != null || x.status === "loading";
    const body = [modelImg(x, { class: "pick-ico" }),
      el("span", { class: "pick-name" }, el("b", {}, shortName(x.id)),
        el("small", {}, loading ? "Loading…" : ready ? (x.id === cur ? "In use" : x.busy ? "Working…" : "Ready to use") : `Needs ${x.npu_usage ?? "?"}% NPU · loads in ~${fmtSec(x.load_time || 60)}`)),
      x.id === cur ? el("span", { class: "pick-check", "aria-hidden": "true" }, "✓") : null];
    const act = ready
      ? el("button", { class: "btn-sm ghost", disabled: !!x.busy, title: x.busy ? "Busy with a job" : "Free its NPU memory", "aria-label": `Unload ${shortName(x.id)}`, onclick: async () => { closePicker(true); await stopModel(x.id); if (document.activeElement === document.body) focusPicker(); } }, "Unload")
      : el("button", { class: "btn-sm accent", disabled: loading, "aria-label": `Load ${shortName(x.id)}`, onclick: () => { closePicker(); choose(x.id); loadModel(x.id); } }, loading ? "Loading…" : "Load");
    return el("div", { class: "pick-row", "aria-current": x.id === cur },
      ready ? el("button", { class: "pick-use", title: x.id === cur ? "In use" : "Use this model", onclick: () => { if (x.id !== cur) choose(x.id); closePicker(true); } }, ...body)
        : el("span", { class: "pick-use" }, ...body), act);
  };
  const loaded = models.filter((x) => x.status === "running"), rest = models.filter((x) => x.status !== "running");
  menu.replaceChildren(...(loaded.length ? [el("div", { class: "pick-head" }, "Loaded"), ...loaded.map(row)] : []),
    ...(rest.length ? [el("div", { class: "pick-head" }, "Not loaded"), ...rest.map(row)] : []));
  menu.anchor = btn; menu.hidden = false; btn.setAttribute("aria-expanded", "true");
  const r = btn.getBoundingClientRect(), below = innerHeight - r.bottom - 12, above = r.top - 12;
  menu.style.width = `${Math.max(r.width, 280)}px`;
  menu.style.left = `${Math.max(8, Math.min(r.left, innerWidth - Math.max(r.width, 280) - 8))}px`;
  const up = below < 260 && above > below;  // near the bottom of the window: open upwards
  menu.style.maxHeight = `${up ? above : below}px`;
  menu.style.top = up ? "" : `${r.bottom + 6}px`; menu.style.bottom = up ? `${innerHeight - r.top + 6}px` : "";
  if (byKey) (menu.querySelector("[aria-current=true] button") || menu.querySelector("button")).focus();
}
function closePicker(refocus) {
  const menu = $("#pickMenu");
  if (!menu || menu.hidden) return;
  menu.hidden = true;
  const btn = menu.anchor;
  btn?.setAttribute("aria-expanded", "false");
  // the panel may have redrawn while it was open: go back to the button that's there now
  if (refocus) (btn?.isConnected ? btn.focus() : focusPicker());
}
const focusPicker = () => [...document.querySelectorAll(".pick-btn")].find((b) => b.offsetParent)?.focus();
/** The picker for a tool; the choice is remembered per tool. */
const toolPicker = (tool, done) => modelPicker(toolModels(tool), modelForTool(tool)?.id, (id) => { store.set("pick." + tool, id); done(); });
function mediaBadges(m) {
  const d = m.desc || "";
  if (m.type === "Text-to-Speech") return [(m.supported || []).includes("Voice Clone") ? "Voice clone" : "Speech", /10 languages/i.test(d) && "10 languages", "Style instructions"].filter(Boolean);
  if (m.type === "ASR") return ["Transcribe", /30 languages/.test(d) && "30 languages", /detect/i.test(d) && "Auto-detect language"].filter(Boolean);
  if (m.type === "Music Generation") return /song|lyric/i.test(m.id + d.slice(0, 300)) ? ["Full songs", "Lyrics + style", "Vocals or instrumental", "Up to 4:30"] : ["Loops", "BPM + key locked", "4 / 8 bars"];
  if (CHAT_TYPES.includes(m.type)) return ["Improve prompts", "Lyrics", "Loop tags", seesImages(m) && "Describe images"].filter(Boolean);
  if (m.type === "Image-to-Text") return [layoutOnly(m) ? "Page layout only" : "Text from images (OCR)"];
  if (m.type === "Text-to-Video") return ["Text or picture → video", /audio/i.test(d) && "With sound"].filter(Boolean);
  if (m.type === "Image-to-3D") return ["Image → 3D", /PBR/.test(d) && "PBR texture", "GLB export"].filter(Boolean);
  return [m.type];
}
/** A card that blocks a tool until its model is loaded (or explains it isn't installed). */
function modelGate(tool, what, picker) {
  if (S.deviceOk === false && S.locked)  // it answers, but needs TiinyOS to unlock it: Settings wouldn't help
    return el("div", { class: "gate" }, el("div", { class: "grow" }, el("b", {}, "Your Tiiny is locked after a restart."), el("p", {}, UNLOCK_HOW)));
  if (S.deviceOk === false)  // no model list to go on: say why rather than "not installed"
    return el("div", { class: "gate" }, el("div", { class: "grow" }, el("b", {}, S.keyRejected ? "Your Tiiny rejected the API key." : "Can't reach the Tiiny."),
      el("p", {}, S.keyRejected ? "Copy the API key (not the device ID) from TiinyOS: avatar → Settings → API Key." : "Check that it's on and the address is right.")),
      el("button", { class: "btn-sm accent", onclick: () => openSettings(false) }, "Settings"));
  const m = modelForTool(tool);
  // Music used to need the models' own ports (blocked over USB); the gateway's /v1/music routes now cover it.
  if (m && (m.status === "running" || picker)) return null;  // with a picker, its card offers Load
  if (!m) return el("div", { class: "gate" }, el("b", {}, `No ${what} model installed.`), el("p", {}, `Download one in the TiinyOS app (${TOOL_TYPES[tool]}).`));
  const loading = S.loading[m.id] != null || m.status === "loading";
  return el("div", { class: "gate" },
    modelImg(m, { class: "gate-ico" }),
    el("div", { class: "grow" }, el("b", {}, `${shortName(m.id)} isn't loaded`), el("p", {}, loading ? "Loading… this takes a few seconds." : `Needs ${m.npu_usage ?? "?"}% NPU · loads in ~${fmtSec(m.load_time || 30)}`)),
    el("button", { class: "btn-sm accent", disabled: loading, onclick: () => loadModel(m.id) }, loading ? "Loading…" : "Load"));
}

/* ---------- chat-model helpers ---------- */
const ASSIST_WAIT = new Map();  // job id -> resolve(text | null)
S.assistBusy = {};
/** Run a helper on the chat model as a queued device job. Resolves to the reply text, or null if it failed or a
    model has to be loaded first (the gate card then shows in `slot` and updates itself as the model loads). */
/** The "load this model" card for a helper, or null when it can run: its model is loaded, or the server may swap it in. */
function helperGate(task) {
  const tool = task === "describe" ? "vision" : "chat", m = modelForTool(tool);
  if (m && m.status !== "running" && swapOn(helperSection(task))) return null;
  return modelGate(tool, tool === "vision" ? "image-reading chat" : "chat");
}
async function assist(task, payload, slot, label) {
  const tool = task === "describe" ? "vision" : "chat";
  const g = helperGate(task);
  if (g) { slot.replaceChildren(el("div", { class: "assist-gate", "data-tool": tool }, g)); return null; }
  slot.replaceChildren();
  if (S.assistBusy[task]) return null;
  S.assistBusy[task] = true;
  try {
    const m = modelForTool(tool);
    let est = { describe: 25, lyrics: 40 }[task] || 15;
    if (m.status !== "running") {  // the server loads it (making room) and puts the user's models back afterwards
      const load = m.load_time || 30;
      est += 2 * load;
      toast(`Swapping models: ${shortName(m.id)} loads first (~${fmtSec(load)}), then your models go back. Turn this off in Settings.`);
    }
    const job = await submitJob("assist", { task, model: m.id, est, ...payload }, label);
    return await new Promise((res) => ASSIST_WAIT.set(job.id, res));
  } catch (e) { toast(e.message, "err"); return null; }
  finally { delete S.assistBusy[task]; }
}
function renderAssistGates() {
  for (const n of document.querySelectorAll(".assist-gate")) {
    const g = modelGate(n.dataset.tool, n.dataset.tool === "vision" ? "image-reading chat" : "chat");
    if (g) n.replaceChildren(g); else { n.replaceChildren(el("div", { class: "hint-line" }, `${shortName(modelForTool(n.dataset.tool).id)} is loaded — run the helper again.`)); n.classList.remove("assist-gate"); }
  }
}
/** A small "Undo" line after a helper replaced text the user wrote. */
function assistUndo(slot, what, undo) {
  slot.replaceChildren(el("div", { class: "assist-note" }, `${what} by ${shortName(modelForTool("chat")?.id || "")}`, el("span", { class: "spacer" }),
    el("button", { class: "linkish", onclick: () => { undo(); slot.replaceChildren(); } }, "Undo"), el("button", { class: "linkish", "aria-label": "Dismiss", onclick: () => slot.replaceChildren() }, "✕")));
}

/* ---------- audio helpers ---------- */
let _actx = null;
const actx = () => (_actx ||= new (window.AudioContext || window.webkitAudioContext)());
async function decodeAudio(blob) { return actx().decodeAudioData(await blob.arrayBuffer()); }
/** Resample to mono 16-bit WAV (what the TTS and ASR containers expect). */
async function toWav(buffer, rate = 24000) {
  const len = Math.ceil(buffer.duration * rate);
  const off = new OfflineAudioContext(1, len, rate);
  const src = off.createBufferSource(); src.buffer = buffer; src.connect(off.destination); src.start();
  const data = (await off.startRendering()).getChannelData(0);
  const out = new DataView(new ArrayBuffer(44 + data.length * 2));
  const w = (o, s) => [...s].forEach((c, i) => out.setUint8(o + i, c.charCodeAt(0)));
  w(0, "RIFF"); out.setUint32(4, 36 + data.length * 2, true); w(8, "WAVE"); w(12, "fmt ");
  out.setUint32(16, 16, true); out.setUint16(20, 1, true); out.setUint16(22, 1, true); out.setUint32(24, rate, true);
  out.setUint32(28, rate * 2, true); out.setUint16(32, 2, true); out.setUint16(34, 16, true); w(36, "data"); out.setUint32(40, data.length * 2, true);
  for (let i = 0; i < data.length; i++) out.setInt16(44 + i * 2, Math.max(-1, Math.min(1, data[i])) * 0x7fff, true);
  return new Blob([out], { type: "audio/wav" });
}
const blobToUri = (b) => new Promise((res, rej) => { const r = new FileReader(); r.onload = () => res(r.result); r.onerror = rej; r.readAsDataURL(b); });
async function fileToWavClip(blob, rate = 24000, maxSec = 0) {
  const buf = await decodeAudio(blob);
  let b = buf;
  if (maxSec && buf.duration > maxSec) {  // trim long references; the model only needs a few seconds
    const n = Math.floor(maxSec * buf.sampleRate);
    b = actx().createBuffer(buf.numberOfChannels, n, buf.sampleRate);
    for (let c = 0; c < buf.numberOfChannels; c++) b.copyToChannel(buf.getChannelData(c).subarray(0, n), c);
  }
  const wav = await toWav(b, rate);
  return { wav, uri: await blobToUri(wav), duration: b.duration, trimmed: b !== buf };
}

/** Mic recorder with a live level meter. */
function createRecorder(onLevel) {
  let rec, stream, chunks = [], raf, analyser;
  return {
    async start() {
      stream = await navigator.mediaDevices.getUserMedia({ audio: { echoCancellation: true, noiseSuppression: true } });
      rec = new MediaRecorder(stream); chunks = [];
      rec.ondataavailable = (e) => e.data.size && chunks.push(e.data);
      rec.start();
      analyser = actx().createAnalyser(); analyser.fftSize = 512;
      actx().createMediaStreamSource(stream).connect(analyser);
      const buf = new Uint8Array(analyser.fftSize);
      const loop = () => { analyser.getByteTimeDomainData(buf); let m = 0; for (const v of buf) m = Math.max(m, Math.abs(v - 128)); onLevel?.(m / 128); raf = requestAnimationFrame(loop); };
      loop();
    },
    stop() {
      return new Promise((res) => {
        cancelAnimationFrame(raf);
        rec.onstop = () => { stream.getTracks().forEach((t) => t.stop()); res(new Blob(chunks, { type: rec.mimeType })); };
        rec.stop();
      });
    },
    get active() { return rec?.state === "recording"; },
  };
}

const peakCache = new Map();
async function peaksFor(url, n = 600) {
  const key = url + "|" + n;
  if (peakCache.has(key)) return peakCache.get(key);
  const p = (async () => {
    const buf = await decodeAudio(await (await fetch(url)).blob());
    const ch = buf.getChannelData(0), step = Math.max(1, Math.floor(ch.length / n)), out = new Float32Array(n);
    for (let i = 0; i < n; i++) { let m = 0; for (let j = i * step, e = Math.min(ch.length, j + step); j < e; j++) m = Math.max(m, Math.abs(ch[j])); out[i] = m; }
    const max = Math.max(...out, 0.01); return { peaks: out.map((v) => v / max), duration: buf.duration };
  })();
  peakCache.set(key, p);
  return p;
}
function drawWave(canvas, peaks, progress = 0) {
  const dpr = window.devicePixelRatio || 1, w = canvas.clientWidth, h = canvas.clientHeight;
  if (!w || !h) return;
  canvas.width = w * dpr; canvas.height = h * dpr;
  const g = canvas.getContext("2d"); g.scale(dpr, dpr); g.clearRect(0, 0, w, h);
  const cs = getComputedStyle(document.documentElement);
  const bar = 3, gap = 1.5, n = Math.floor(w / (bar + gap));
  for (let i = 0; i < n; i++) {
    const v = peaks[Math.floor(i / n * peaks.length)] || 0, bh = Math.max(2, v * (h - 4));
    g.fillStyle = i / n < progress ? cs.getPropertyValue("--accent").trim() : cs.getPropertyValue("--line-2").trim();
    g.beginPath(); g.roundRect?.(i * (bar + gap), (h - bh) / 2, bar, bh, 1.5) ?? g.rect(i * (bar + gap), (h - bh) / 2, bar, bh); g.fill();
  }
}
/** Waveform player: play/pause, click-to-seek, time readout. */
let currentAudio = null;
function makePlayer(url, { big = false } = {}) {
  const audio = new Audio(url); audio.preload = "metadata";
  const canvas = el("canvas", { class: "wave" + (big ? " big" : ""), role: "slider", tabindex: "0", "aria-label": "Seek", "aria-valuemin": "0", "aria-valuenow": "0", "aria-valuetext": "0:00" });
  const btn = el("button", { class: "play", title: "Play / pause (space)" }, icon("i-play"));
  const time = el("span", { class: "ptime" }, "0:00");
  const mmss = (s) => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, "0")}`;
  let peaks = null;
  const sync = () => {  // the slider's value, updated once a second rather than on every frame
    const t = Math.floor(audio.currentTime);
    if (canvas.dataset.t === String(t)) return;
    canvas.dataset.t = t; canvas.setAttribute("aria-valuenow", t); canvas.setAttribute("aria-valuetext", `${mmss(t)} of ${mmss(audio.duration || 0)}`);
    if (audio.duration) canvas.setAttribute("aria-valuemax", Math.floor(audio.duration));
  };
  const redraw = () => { if (peaks) drawWave(canvas, peaks, audio.duration ? audio.currentTime / audio.duration : 0); sync(); };
  peaksFor(url).then((p) => { peaks = p.peaks; time.textContent = `0:00 / ${mmss(p.duration)}`; redraw(); }).catch(() => {});
  const tick = () => { redraw(); time.textContent = `${mmss(audio.currentTime)} / ${mmss(audio.duration || 0)}`; if (!audio.paused) requestAnimationFrame(tick); };
  audio.onplay = () => { if (currentAudio && currentAudio !== audio) currentAudio.pause(); currentAudio = audio; btn.replaceChildren(icon("i-pause")); tick(); };
  audio.onpause = audio.onended = () => { btn.replaceChildren(icon("i-play")); redraw(); };
  btn.onclick = () => (audio.paused ? audio.play() : audio.pause());
  canvas.onclick = (e) => { if (!audio.duration) return; const r = canvas.getBoundingClientRect(); audio.currentTime = (e.clientX - r.left) / r.width * audio.duration; redraw(); if (audio.paused) audio.play(); };
  canvas.onkeydown = (e) => {  // keyboard seeking: arrows 5 s, Home/End
    const d = audio.duration; if (!d || e.ctrlKey || e.metaKey || e.altKey) return;
    const t = { ArrowLeft: audio.currentTime - 5, ArrowRight: audio.currentTime + 5, Home: 0, End: d }[e.key];
    if (t == null) return;
    e.preventDefault(); audio.currentTime = Math.max(0, Math.min(d, t)); redraw();
  };
  new ResizeObserver(redraw).observe(canvas);
  const node = el("div", { class: "player" + (big ? " big" : "") }, btn, canvas, time);
  node.audio = audio;
  return node;
}
document.addEventListener("keydown", (e) => {
  if (e.key === " " && S.ws !== "image" && !e.target.closest("input, textarea, select, button") && currentAudio) {
    e.preventDefault(); currentAudio.paused ? currentAudio.play() : currentAudio.pause();
  }
});

/* ---------- small shared UI ---------- */
/** Two-click delete for a gallery thumbnail: first click arms it ("Delete?"), second deletes. */
function thumbDel(onDelete) {
  const b = el("button", { class: "thumb-del", type: "button", title: "Delete", "aria-label": "Delete" }, "✕");  // a sibling of the thumbnail button, never inside it
  let timer = null;
  const act = (ev) => {
    ev.stopPropagation(); ev.preventDefault();
    if (!b.classList.contains("armed")) {
      b.classList.add("armed"); b.textContent = "Delete?"; b.setAttribute("aria-label", "Delete? Press again to confirm");
      timer = setTimeout(() => { b.classList.remove("armed"); b.textContent = "✕"; b.setAttribute("aria-label", "Delete"); }, 3000);
      return;
    }
    clearTimeout(timer); b.textContent = "…"; onDelete();
  };
  b.addEventListener("click", act);
  return b;
}
function mediaThumb(e, current, onclick) {
  const icons = { speech: "i-voice", loop: "i-wave", song: "i-music", transcript: "i-text", mesh: "i-cube", video: "i-film" };
  const labels = { speech: "VOICE", loop: "LOOP", song: "SONG", transcript: "TEXT", mesh: "3D", video: "VIDEO" };
  const title = e.params?.text || e.params?.prompt || e.params?.caption || e.text || "";
  const t = el("button", { class: "thumb media-thumb", "aria-current": current, title, "aria-label": `${labels[e.kind] || e.kind} ${title}`.trim(), onclick });
  if (e.kind === "mesh" && e.sources?.[0]) t.append(el("img", { src: "/images/" + e.sources[0], alt: "", loading: "lazy" }));
  else if (e.kind === "video") t.append(el("video", { src: "/images/" + e.outputs[0] + "#t=0.5", muted: true, preload: "metadata" }));
  else t.append(icon(icons[e.kind] || "i-wave"));
  t.append(el("span", { class: "badge" }, labels[e.kind] || e.kind));
  if (e.duration) t.append(el("span", { class: "multi" }, fmtSec(e.duration)));
  return t;
}
function pendingCard(j, onCancel) {
  const t = j.started ? (Date.now() / 1000 - j.started) : 0;
  const est = j.est || 0;
  const pct = j.progress || (est ? Math.min(92, t / est * 92) : 0);
  return el("div", { class: "pending media-pending" },
    el("div", { class: "pending-inner" },
      el("div", { class: "big" }, j.status === "queued" ? "Waiting" : fmtSec(t)),
      el("div", { class: "sub" }, j.status === "queued" ? `#${j.queue_pos || 1} in the device queue — the Tiiny runs one job at a time` : j.message || j.status),
      el("div", { class: "bar" }, el("i", { style: `width:${pct.toFixed(1)}%` })),
      el("div", { class: "prompt-echo" }, j.label || j.kind),
      el("button", { class: "tb cancel", disabled: !!j.stopping, title: j.stopping ? STOPPING : "", ...onPress((b) => onCancel ? onCancel(j) : stopButton(b, j.id)) }, j.stopping ? "Stopping…" : "Cancel")));
}

/* ---------- panels: Audio, 3D and Video ---------- */
/** A seed box with a dice button; obj.seed holds the digits ("" = new each run). */
function seedField(obj) {
  const inp = el("input", { class: "input", inputmode: "numeric", placeholder: "random", value: obj.seed, oninput: (e) => { obj.seed = e.target.value.replace(/\D/g, ""); e.target.value = obj.seed; } });
  return el("div", { class: "field" }, el("div", { class: "label" }, "Seed", el("span", { class: "hint" }, "blank = new each run")),
    el("div", { class: "seed-row" }, inp, el("button", { class: "toggle-btn", title: "Roll", onclick: () => { obj.seed = String(Math.floor(Math.random() * 2 ** 31)); inp.value = obj.seed; } }, icon("i-dice"))));
}
/** One picture to drop, paste or browse for, shown with a remove button once set. take gets the files. */
function imageSlot({ img, clear, cls, key, zone, title, hint, take }) {
  if (img) return el("div", { class: cls }, el("img", { src: img.uri, alt: "" }), el("button", { class: "x", title: "Remove", "aria-label": "Remove picture", onclick: clear }, "✕"));
  const d = el("button", { class: "drop", onclick: () => { const i = el("input", { type: "file", accept: "image/*" }); i.onchange = () => i.files[0] && take([i.files[0]]); i.click(); } },
    icon("i-upload"), el("span", {}, el("b", {}, title), hint));
  return fileZone(d, { key, accept: /^image\//, what: "image", label: zone, take });
}
/** What the three panels share, mixed into each workspace object: { ...mediaWS({...}), … }. c: { kind (also the
 *  workspace and gallery name), p (id prefix), st (its state, st.view = {id} | {job}), title, empty, own(entry),
 *  jobs(job), queued(job) -> { label, run, title, view() }, gate() -> [tool, what], thumb(el, entry), early(job),
 *  fresh }. A panel still supplies mount (via shell), render, renderFoot, renderStage, submit. */
function mediaWS(c) {
  const { kind, p, st } = c;
  return {
    shell(root, { top = [], ico, label, est, foot, bar = [] }) {
      root.replaceChildren(
        el("aside", { class: "composer" },
          el("div", { class: "composer-scroll" }, ...top, el("div", { id: p + "Gate" }), el("div", { id: p + "Panel" })),
          el("div", { class: "composer-foot" },
            el("button", { class: "go", id: p + "Go", onclick: () => this.submit() }, icon(ico), typeof label === "string" ? el("span", {}, label) : label, el("span", { class: "est", id: est })),
            el("div", { class: "foot-meta" }, ...foot))),
        el("section", { class: "stage-col" },
          el("div", { class: "stage-head", id: p + "Head" }),
          el("div", { class: "stage", id: p + "Stage" }),
          el("div", { class: "strip-wrap" },
            el("div", { class: "strip-head" }, c.title + " ", el("span", { class: "count", id: p + "Count" }), el("span", { id: "galBar-" + kind }), ...bar),
            el("div", { class: "strip", id: p + "Strip" }))));
    },
    seg(items, val, set, key) { return el("div", { class: "seg small" }, items.map(([v, label]) => el("button", { "aria-pressed": val === v, onclick: () => { set(v); store.set(key, v); this.render(); } }, label))); },
    renderGate() {
      const pick = toolPicker(c.gate()[0], () => this.render()), g = modelGate(...c.gate(), !!pick);
      $(`#${p}Gate`).replaceChildren(...[pick, g].filter(Boolean));
      this.renderFoot();
    },
    /** The tool's model and whether it's loaded; also writes the "name · not loaded" line in the footer. */
    footModel(tool, extra = "") {
      const m = modelForTool(tool), ready = m?.status === "running";
      $(`#${p}Meta`).textContent = m ? shortName(m.id) + (ready ? "" : " · not loaded") + extra : "";
      return { m, ready };
    },
    entries() { return S.history.filter(c.own); },
    /** The queued or running job on show, if any; a finished one drops back to the newest result. */
    liveJob() {
      if (!st.view?.job) return null;
      const j = S.jobs.find((x) => x.id === st.view.job);
      if (j && (j.status === "queued" || j.status === "running")) return j;
      st.view = null;
      return null;
    },
    /** The result on show: the chosen one, else the newest when nothing is chosen. */
    current(list) { return (st.view && list.find((x) => x.id === st.view.id)) || (!st.view && list[0]); },
    jobDone(j) {
      if (c.early?.(j)) return;
      if (st.view?.job === j.id) st.view = j.status === "done" ? { id: j.entry.id, ...(c.fresh && { fresh: true }) } : null;
      if (this.mounted && S.ws === kind) { this.renderStage(); this.renderStrip(); }
    },
    renderJobs() { if (st.view?.job) this.renderStage(); this.renderStrip(); },
    renderStrip() {
      const items = [], show = () => { this.renderStage(); this.renderStrip(); };
      for (const j of S.jobs.filter((x) => c.jobs(x) && (x.status === "queued" || x.status === "running"))) {
        const q = c.queued(j);
        items.push(el("button", { class: "thumb queued" + (q.run ? " run" : ""), "aria-current": st.view?.job === j.id, title: q.title, "aria-label": `${q.label}: ${q.title}`, onclick: () => { st.view = q.view?.() || { job: j.id }; show(); } }, q.label));
      }
      const list = this.entries(), shown = galleryFilter(kind, list);
      $("#galBar-" + kind).replaceChildren(galleryBar(kind, shown, () => this.renderStrip()));
      for (const e of shown) {
        const t = mediaThumb(e, st.view?.id === e.id, () => { st.view = { id: e.id }; show(); });
        c.thumb?.(t, e);
        items.push(decorateThumb(kind, t, e, () => this.renderStrip(), thumbDel(() => this.del(e))));
      }
      $(`#${p}Strip`).replaceChildren(...(items.length ? items : [el("div", { class: "strip-empty" }, c.empty)]));
      $(`#${p}Count`).textContent = list.length || "";
    },
    /** Deleting the result on show moves the display to its neighbour (the older one, else the newer), as in Image. */
    async del(e) {
      try { await api("/api/history/" + e.id, { method: "DELETE" }); } catch (err) { return toast(err.message, "err"); }
      const list = this.entries(), i = list.indexOf(e);
      S.history.splice(S.history.indexOf(e), 1);
      if (st.view?.id === e.id) { const next = list[i + 1] || list[i - 1]; st.view = next ? { id: next.id } : null; }
      this.renderStage(); this.renderStrip();
    },
  };
}

// DOMContentLoaded fires after every classic script (audio.js, mesh.js) has run.
document.addEventListener("DOMContentLoaded", () => { setWs(S.ws); pollJobs(); });
