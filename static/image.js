"use strict";
const ASPECTS = [
  { id: "1:1", w: 1, h: 1 }, { id: "4:3", w: 4, h: 3 }, { id: "3:4", w: 3, h: 4 },
  { id: "16:9", w: 16, h: 9 }, { id: "9:16", w: 9, h: 16 }, { id: "src", w: 0, h: 0, label: "Source" },
];
const CREATE_CHIPS = ["cinematic lighting", "35mm photo", "watercolor", "isometric 3D", "studio product shot", "golden hour"];
const EDIT_CHIPS = ["Change the background to ", "Make it night time", "Turn it into a watercolor painting", "Remove the ", "Add ", "Change the color of the "];
/* ---------- size math ---------- */
// Sizes a model's description says it supports (e.g. "512x512 is native … verified: 432x768, 768x432, …").
// Off-list sizes can come out with seams, so when a model lists sizes the UI only offers those.
function modelSizeList(m) {
  const d = m?.desc || "";
  if (!/resolution|sizes?\b/i.test(d)) return null;
  const seen = new Set(), out = [];
  for (const [, w, h] of d.matchAll(/\b(\d{3,4})\s*[x×]\s*(\d{3,4})\b/g)) {
    const k = `${w}x${h}`;
    if (!seen.has(k) && +w >= 256 && +h >= 256) { seen.add(k); out.push([+w, +h]); }
  }
  return out.length >= 2 ? out : null;
}
const gcd = (a, b) => (b ? gcd(b, a % b) : a);
function aspectKey(w, h) {
  const g = gcd(w, h), k = `${w / g}:${h / g}`;
  return { "8:5": "16:10", "5:8": "10:16" }[k] || k;
}
/** Shape choices for the active model: its verified aspect ratios, or the generic set. */
function shapeOptions() {
  const list = modelSizeList(modelById(S.model));
  if (!list) return { verified: false, shapes: ASPECTS.filter((a) => a.id !== "src") };
  const groups = new Map();
  for (const [w, h] of list) {
    const id = aspectKey(w, h);
    if (!groups.has(id)) { const [a, b] = id.split(":").map(Number); groups.set(id, { id, w: a, h: b, sizes: [] }); }
    groups.get(id).sizes.push([w, h]);
  }
  const shapes = [...groups.values()];
  shapes.forEach((g) => g.sizes.sort((p, q) => p[0] * p[1] - q[0] * q[1]));
  const skew = (g) => Math.round(Math.abs(Math.log(g.w / g.h)) * 1e6);  // rounded so 16:10 and 10:16 tie
  shapes.sort((p, q) => skew(p) - skew(q) || q.w / q.h - p.w / p.h);  // squarest first, landscape before portrait
  return { verified: true, shapes };
}
function sizeFor() {
  const { verified, shapes } = shapeOptions();
  const src = S.sources[0];
  if (verified) {
    let g = shapes.find((x) => x.id === S.aspect);
    if (S.aspect === "src" && src) {  // closest verified ratio to the source image
      const r = src.w / src.h;
      g = shapes.reduce((b, x) => Math.abs(Math.log(x.w / x.h / r)) < Math.abs(Math.log(b.w / b.h / r)) ? x : b);
    }
    g ||= shapes[0];
    return g.sizes.reduce((b, s) => Math.abs(Math.max(...s) - S.res) < Math.abs(Math.max(...b) - S.res) ? s : b);
  }
  let a = ASPECTS.find((x) => x.id === S.aspect) || ASPECTS[0];
  let rw = a.w, rh = a.h;
  if (a.id === "src") { if (!src) { rw = 1; rh = 1; } else { rw = src.w; rh = src.h; } }
  const px = S.res * S.res;
  let w = Math.sqrt(px * rw / rh), h = px / w;
  const r64 = (v) => Math.max(256, Math.round(v / 64) * 64);  // diffusion models want multiples of 64
  return [r64(w), r64(h)];
}
/** Resolution buttons: the verified sizes for the chosen shape, or the generic tiers. */
function resolutionOptions() {
  const { verified, shapes } = shapeOptions();
  if (!verified) return [512, 768, 1024].map((r) => ({ res: r, label: String(r) }));
  let g = shapes.find((x) => x.id === S.aspect);
  if (S.aspect === "src" && S.sources[0]) { const [w, h] = sizeFor(); g = shapes.find((x) => x.id === aspectKey(w, h)); }
  g ||= shapes[0];
  return g.sizes.map(([w, h]) => ({ res: Math.max(w, h), label: String(Math.max(w, h)), title: `${w} × ${h}` }));
}

/* ---------- timing estimate ---------- */
function units(params) {
  const [w, h] = (params.size || "512x512").split("x").map(Number);
  return (w * h) / (512 * 512) * (params.steps || 30) / 30 * (params.n || 1) * (params.image ? 1.15 : 1);
}
function estimate(params) {
  const per = S.timing[params.model];
  if (per == null) return /turbo/i.test(params.model || "") ? 10 * units(params) : 75 * units(params);
  return per * units(params);
}
function learn(params, seconds) {
  const u = units(params); if (!u || !seconds) return;
  const prev = S.timing[params.model];
  S.timing[params.model] = prev == null ? seconds / u : prev * 0.5 + (seconds / u) * 0.5;
  store.set("timing", S.timing);
}
/* Settings remembered per image model (steps, guidance, size, negative prompt). */
function saveModelSettings(id) {
  if (!id) return;
  store.set("ms." + id, { stepsAuto: $("#stepsAuto").checked, steps: $("#steps").value, cfgAuto: $("#cfgAuto").checked, cfg: $("#cfg").value,
    negative: $("#negative").value, aspect: S.aspect === "src" ? S.prevAspect : S.aspect, res: S.res });
}
function loadModelSettings(id) {
  const m = store.get("ms." + id, null);
  if (!m) return;
  $("#stepsAuto").checked = m.stepsAuto; $("#steps").value = m.steps; $("#steps").disabled = m.stepsAuto;
  $("#cfgAuto").checked = m.cfgAuto; $("#cfg").value = m.cfg; $("#cfg").disabled = m.cfgAuto;
  $("#negative").value = m.negative || "";
  if (m.aspect && S.aspect !== "src") S.aspect = m.aspect;
  if (m.res) S.res = m.res;
  syncSliders();
}
function selectModel(id) {
  if (S.model && S.model !== id) saveModelSettings(S.model);
  const changed = S.model !== id;
  S.model = id; store.set("model", id);
  if (changed) loadModelSettings(id);
  if (S.mode === "edit" && !canEditModel(id)) setMode("create");
  renderModelBtn(); renderDrawer(); renderComposer();
}

const imageHistory = () => S.history.filter((e) => (e.kind || "image") === "image");
async function loadHistory() {
  try { S.history = await api("/api/history"); } catch (e) { toast(e.message, "err"); }
  const ih = imageHistory();
  if (!S.view && ih[0]) S.view = { id: ih[0].id, idx: 0 };
  if (typeof onHistoryLoaded === "function") onHistoryLoaded();
  renderStrip(); renderStage();
}

/* ---------- sources ---------- */
function readImage(file, max = 1536) {
  return new Promise((res, rej) => {
    const fr = new FileReader();
    fr.onerror = rej;
    fr.onload = () => {
      const img = new Image();
      img.onload = () => {
        const scale = Math.min(1, max / Math.max(img.naturalWidth, img.naturalHeight));
        let uri = fr.result, w = img.naturalWidth, h = img.naturalHeight;
        if (scale < 1 || !/^data:image\/(png|jpeg|webp)/.test(uri)) {
          w = Math.round(w * scale); h = Math.round(h * scale);
          const c = Object.assign(document.createElement("canvas"), { width: w, height: h });
          c.getContext("2d").drawImage(img, 0, 0, w, h);
          uri = c.toDataURL("image/png");
        }
        res({ uri, w, h, name: file.name || "pasted" });
      };
      img.onerror = () => rej(new Error("Not a readable image"));
      img.src = fr.result;
    };
    fr.readAsDataURL(file);
  });
}
async function addFiles(files, here = false) {  // here: dropped on the picture tray itself, not the page
  if (S.mode === "text" && files.length) {  // read right away: a picture (kept large so small print survives) or a document
    const f = files[0];
    if (!browserImage(f)) { S.sources = []; S.textDoc = f; renderComposer(); return readDocument(f); }
    try { S.sources = [await readImage(f, 3072)]; S.textDoc = null; } catch (e) { return toast(e.message, "err"); }
    renderComposer(); return ocrImage(S.sources[0].uri);
  }
  const list = [...files].filter((f) => /^image\//.test(f.type));
  if (!list.length) return;
  if (S.mode === "create" && !(here && caps(modelById(S.model)).edit)) setMode("edit");  // dropped on the page: edit it
  for (const f of list) {
    const max = Math.max(1, caps(modelById(S.model)).maxRefs);
    if (S.sources.length >= max) { toast(max === 1 ? "This model takes one source image" : `Up to ${max} images for this model`); break; }
    try { S.sources.push(await readImage(f)); } catch (e) { toast(e.message, "err"); }
  }
  if (S.mode === "create") renderComposer(); else setMode("edit");
}
async function useAsSource(url) {
  const blob = await (await fetch(url)).blob();
  setMode("edit");
  S.sources = [await readImage(new File([blob], url.split("/").pop(), { type: blob.type }))];
  if (S.aspect !== "src") { S.prevAspect = S.aspect; }
  S.aspect = "src";
  setMode("edit");
  const p = $("#prompt"); p.value = ""; p.focus(); autoGrow();
  toast("Loaded as source — describe the change");
}

/* ---------- composer render ---------- */
function setMode(m) {
  // Create keeps its own pictures (references for a new image), apart from Edit's and Text's source
  if ((m === "create") !== (S.mode === "create")) [S.sources, S.otherSources] = [S.otherSources || [], S.sources];
  S.mode = m;
  if (m === "create" && S.aspect === "src") S.aspect = S.prevAspect || "1:1";
  if (m === "edit" && S.sources.length && S.aspect !== "src" && !S.userPickedAspect) S.aspect = "src";
  renderComposer();
}

function renderComposer() {
  // extras.js (OCR) loads after this script; Text mode shows once an OCR model is installed
  const ocrOn = typeof ocrModels === "function" && ocrModels().length > 0;
  if (S.mode === "text" && !ocrOn) S.mode = "create";
  const edit = S.mode === "edit", text = S.mode === "text";
  const C = caps(modelById(S.model)), refs = S.mode === "create" && C.edit;  // Create takes reference pictures
  $("#modeText").hidden = !ocrOn;
  $("#modeText").setAttribute("aria-pressed", text);
  $(".composer").classList.toggle("ocr-mode", text);
  $("#modeCreate").setAttribute("aria-pressed", !edit);
  $("#modeEdit").setAttribute("aria-pressed", edit);
  $("#sourceField").hidden = !edit && !text && !refs;
  $("#srcTitle").textContent = refs ? "References" : "Source image";
  $("#promptLabel").textContent = edit ? "Describe the change" : "Describe the image";
  $("#prompt").placeholder = edit ? "Change the background to a sunny beach, keep the subject exactly the same"
    : refs && S.sources.length ? "A new scene: the mug from image 1 on a café table, golden hour" : "A watercolor fox in a misty pine forest, soft morning light";

  // sources
  const box = $("#sources");
  box.replaceChildren(...S.sources.map((s, i) => el("div", { class: "src" },
    el("img", { src: s.uri, alt: "" }),
    el("span", { class: "tag" }, text ? "TEXT" : !edit ? "IMAGE " + (i + 1) : (i === 0 ? "SOURCE" : "REF " + i) + (s.orig ? " · MARKED" : "")),
    edit && C.annotate ? el("button", { class: "mark", title: "Circle or paint the area to change", onclick: () => openAnnot(i) }, s.orig ? "Re-mark" : "Mark area") : null,
    el("button", { class: "x", title: "Remove", "aria-label": `Remove picture ${i + 1}`, onclick: () => { S.sources.splice(i, 1); if (!S.sources.length && S.aspect === "src") S.aspect = S.prevAspect || "1:1"; renderComposer(); } }, "✕"))));
  if (text && S.textDoc) box.append(el("div", { class: "src doc", title: S.textDoc.name },
    el("b", {}, (S.textDoc.name.match(/\.(\w{1,5})$/)?.[1] || "file").toUpperCase()), el("small", {}, S.textDoc.name),
    el("button", { class: "x", title: "Remove", "aria-label": `Remove ${S.textDoc.name}`, onclick: () => { S.textDoc = null; renderComposer(); } }, "✕")));
  if (!text) S.textDoc = null;
  $("#fileInput").accept = text ? DOC_ACCEPT : "image/png,image/jpeg,image/webp";
  const drop = el("button", { class: "drop" + (S.sources.length || refs ? " compact" : ""), onclick: () => $("#fileInput").click() },
    icon("i-upload"),
    text ? el("span", {}, el("b", {}, "Drop, paste (Ctrl+V) or browse a picture or document"), "Photos, scans, screenshots, PDF, TIFF, Word, PowerPoint, Excel, OpenDocument, EPUB, RTF, HTML or text. Read right away, never saved.")
      : refs ? el("span", {}, S.sources.length ? "Add picture" : "Add a reference (optional)") : S.sources.length ? el("span", {}, "Add ref") : el("span", {}, el("b", {}, "Drop, paste (Ctrl+V) or click to browse"), "PNG · JPG · WebP — or pick one from the gallery below"));
  const zone = fileZone(drop, { key: "img-src", accept: { test: (t) => S.mode === "text" || /^image\//.test(t) }, what: "image", label: srcLabel, take: (f) => addFiles(f, true) });
  if (S.sources.length + (text && S.textDoc ? 1 : 0) < (text ? 1 : Math.max(1, C.maxRefs))) box.append(zone);
  $("#srcHint").textContent = text ? "drop, paste or browse" : refs ? `optional, up to ${C.maxRefs} · call them "image 1", "image 2"` : C.maxRefs > 1 ? `source + up to ${C.maxRefs - 1} references` : "drop, paste, or pick from gallery";

  // chips
  $("#chips").replaceChildren(...(edit ? EDIT_CHIPS : CREATE_CHIPS).map((c) => el("button", { class: "chip", onclick: () => insertChip(c, edit) }, edit ? c.trim().replace(/ (the|to)$/, " …").replace(/ $/, " …") : "+ " + c)));

  // model-specific features
  const feats = [];
  if (C.transparent) feats.push(el("button", { class: "chip", "aria-pressed": S.transparent, title: "Ask for a real alpha channel (RGBA PNG)", onclick: () => { S.transparent = !S.transparent; renderComposer(); } }, "◩ Transparent background"));
  if (edit && C.annotate && S.sources[0] && !S.sources[0].orig) feats.push(el("button", { class: "chip", onclick: () => openAnnot(0) }, "✎ Mark the area to change"));
  if (edit && S.sources[0]?.orig) feats.push(el("button", { class: "chip", onclick: () => insertChip("In the area marked in red, ", true) }, "In the marked area …"));
  $("#features").replaceChildren(...feats);
  const am = modelById(S.model);
  // several image models: pick one here; one that isn't loaded loads (the drawer shows its progress)
  const pick = am && modelPicker(S.models, am.id, (id) => { if (modelById(id)?.status === "running") selectModel(id); else { loadModel(id); renderComposer(); } });
  $("#modelHint").replaceChildren(...(am ? [pick || el("span", {}, el("b", {}, shortName(am.id))), el("span", {}, (pick ? "" : " · ") + capsLine(am))] : S.deviceOk ? [el("span", {}, "No image model loaded — "), el("a", { href: "#", style: "color:var(--accent-2)", onclick: (e) => { e.preventDefault(); openDrawer(); } }, "load one")] : []));
  $("#modeEdit").classList.toggle("locked", !!am && !C.edit);
  $("#modeEdit").title = am && !C.edit ? `${shortName(am.id)} can't edit images` : "Edit an existing image";

  // aspects: the active model's verified shapes when it lists any
  const shapeSet = shapeOptions();
  if (S.aspect !== "src" && !shapeSet.shapes.some((a) => a.id === S.aspect)) S.aspect = shapeSet.shapes[0].id;
  const shapeList = [...shapeSet.shapes, ASPECTS.find((a) => a.id === "src")];
  $("#aspects").style.gridTemplateColumns = `repeat(${Math.min(shapeList.length - (edit && S.sources.length ? 0 : 1), 6)}, 1fr)`;
  $("#aspects").replaceChildren(...shapeList.map((a) => {
    let w = a.w, h = a.h;
    if (a.id === "src") { const s = S.sources[0]; w = s ? s.w : 1; h = s ? s.h : 1; }
    const k = 18 / Math.max(w, h);
    return el("button", { class: "aspect", "aria-pressed": S.aspect === a.id, hidden: a.id === "src" && !(edit && S.sources.length), title: a.id === "src" ? "Match the source image's shape" : a.id,
      onclick: () => { S.aspect = a.id; S.userPickedAspect = true; if (a.id !== "src") store.set("aspect", a.id); renderComposer(); } },
      el("i", { style: `width:${Math.max(6, w * k)}px;height:${Math.max(6, h * k)}px` }), a.label || a.id);
  }));
  const [w, h] = sizeFor();
  $("#sizeReadout").textContent = `${w} × ${h}${shapeSet.verified ? " · model's verified sizes" : ""}`;
  const resOpts = resolutionOptions(), curRes = Math.max(w, h);
  $("#resSeg").replaceChildren(...resOpts.map((o) => el("button", { "aria-pressed": shapeSet.verified ? o.res === curRes : o.res === S.res, title: o.title || "",
    onclick: () => { S.res = o.res; store.set("res", S.res); renderComposer(); } }, o.label)));
  $("#nVal").textContent = S.n;

  // advanced summary
  const bits = [];
  if ($("#negative").value.trim()) bits.push("neg");
  if (S.seedLocked || $("#seed").value) bits.push("seed " + ($("#seed").value || "?"));
  if (!$("#stepsAuto").checked) bits.push($("#steps").value + " steps");
  if (!$("#cfgAuto").checked) bits.push("cfg " + $("#cfg").value);
  $("#advSum").textContent = bits.join(" · ") || "auto";
  $("#seedHint").textContent = S.seedLocked ? "locked" : $("#seed").value ? "used once, then re-rolled" : "new each run";

  // capability notice
  const notice = $("#notice");
  const editRunning = S.models.find((m) => m.status === "running" && canEditModel(m.id));
  const editInstalled = S.models.find((m) => canEditModel(m.id));
  const showNotice = (text, act, fn) => { notice.classList.add("show"); $("#noticeText").textContent = text; $("#noticeAct").textContent = act || ""; $("#noticeAct").onclick = fn || null; };
  notice.classList.remove("show");
  if (S.deviceOk === false) showNotice(S.keyRejected ? "Your Tiiny rejected the API key." : "Can't reach the device.", S.keyRejected ? "Fix it in Settings" : "Check settings", () => openSettings(false));
  else if (text) { /* OCR uses its own model; ocrImage loads it if needed */ }
  else if (S.deviceOk && !S.model) showNotice(Object.keys(S.loading).length ? "Loading a model…" : "No image model is loaded on the device.", "Open models", openDrawer);
  else if (edit && S.model && !C.edit) {
    if (editRunning) showNotice(`${shortName(S.model)} can only create images.`, "Use " + shortName(editRunning.id), () => selectModel(editRunning.id));
    else if (editInstalled) showNotice(`${shortName(S.model)} can only create images. ${shortName(editInstalled.id)} can edit but isn't loaded.`, S.loading[editInstalled.id] ? "Loading…" : "Load it", () => loadModel(editInstalled.id));
    else showNotice(`${shortName(S.model)} can only create images, and no edit-capable model is installed.`);
  }

  // go button
  const p = buildParams(true);
  const go = $("#go");
  go.disabled = text ? !S.deviceOk || !(S.sources.length || S.textDoc) : !S.model || !S.deviceOk || !$("#prompt").value.trim() || (edit && (!S.sources.length || !C.edit));
  $("#goLabel").textContent = text ? (S.sources.length || S.textDoc ? "Read text again" : "Read text") : edit ? (S.queue.length || S.running ? "Queue edit" : "Apply edit") : (S.queue.length || S.running ? "Queue" : "Generate");
  $("#goEst").textContent = text ? "" : S.model ? "~" + fmtSec(estimate(p)) : "";
  renderStatus();
}

function insertChip(c, edit) {
  const p = $("#prompt");
  if (edit) { p.value = c; p.focus(); p.setSelectionRange(c.length, c.length); }
  else { p.value = p.value.trim() ? p.value.trim().replace(/[,.]?$/, ", ") + c : c; p.focus(); }
  autoGrow(); renderComposer();
}

function buildParams(preview = false) {
  const [w, h] = sizeFor();
  let prompt = $("#prompt").value.trim();
  if (S.transparent && caps(modelById(S.model)).transparent && !/transparent background/i.test(prompt)) prompt = prompt.replace(/[.\s]*$/, ". ") + "The image has an alpha channel and a transparent background.";
  const p = { model: S.model, prompt, size: `${w}x${h}`, n: S.n };
  const neg = $("#negative").value.trim(); if (neg) p.negative_prompt = neg;
  if (!$("#stepsAuto").checked) p.steps = Number($("#steps").value);
  if (!$("#cfgAuto").checked) p.guidance_scale = Number($("#cfg").value);
  if (!preview) {
    let seed = $("#seed").value.trim();
    if (!/^\d+$/.test(seed)) seed = String(Math.floor(Math.random() * 2 ** 31));
    p.seed = Number(seed);
    if (!S.seedLocked) $("#seed").value = "";
    else $("#seed").value = seed;
  }
  if (S.mode === "edit" || (S.mode === "create" && S.sources.length && caps(modelById(S.model)).edit)) p.image = true;
  return p;
}

/* ---------- queue (server-side jobs, shared with Audio and 3D) ---------- */
const LOCALJ = {};   // job id -> full params + source images, for jobs submitted from this tab
const IMGJ = {};     // job id -> stable view object (render code compares by identity)
let imgSig = "";
async function submitImage() {
  if ($("#go").disabled) return;
  if (S.mode === "text") return S.textDoc ? readDocument(S.textDoc) : ocrImage(S.sources[0].uri);
  const params = buildParams();
  saveModelSettings(S.model);
  const refs = S.mode === "create" && params.image;  // a new picture that uses the pictures as guides
  const images = S.mode === "edit" || refs ? S.sources.map((s) => s.uri) : [];
  const { image, ...body } = params;
  try {
    const j = await api("/api/jobs", { method: "POST", body: JSON.stringify({ kind: "image", ...body, images, ...(refs ? { mode: "refs" } : {}), est: estimate(params),
      label: (refs ? "Compose: " : images.length ? "Edit: " : "Image: ") + params.prompt.slice(0, 40) }) });
    LOCALJ[j.id] = { params, images };
    S.jobs.push(j);
    S.view = { job: j.id };
    syncImageJobs(true);
    if (typeof pollJobs === "function") pollJobs();
  } catch (e) { toast(e.message, "err"); }
}
/** Rebuild S.running / S.queue from the server's job list. */
function syncImageJobs(force = false) {
  const live = (S.jobs || []).filter((j) => j.kind === "image" && (j.status === "queued" || j.status === "running"));
  const view = (j) => Object.assign(IMGJ[j.id] ||= { jid: j.id }, {
    params: LOCALJ[j.id]?.params || j.params || {}, images: LOCALJ[j.id]?.images || [],
    est: j.est || estimate(j.params || {}), started: j.started ? j.started * 1000 : Date.now(), pos: j.queue_pos, status: j.status });
  S.running = live.filter((j) => j.status === "running").map(view)[0] || null;
  S.queue = live.filter((j) => j.status === "queued").map(view);
  const sig = live.map((j) => j.id + j.status + (j.queue_pos || "")).join();
  if (force || sig !== imgSig) { imgSig = sig; renderAll(); }
}
/** Called by shared.js when an image job finishes. */
function imageJobDone(j) {
  if (j.status === "done" && j.entry) {
    learn(LOCALJ[j.id]?.params || j.entry.params, j.entry.seconds);
    if (S.view?.job === j.id) S.view = { id: j.entry.id, idx: 0, fresh: true };
    $("#lastMeta").textContent = `last: ${fmtSec(j.entry.seconds)}`;
    notify(j.entry);
  } else if (S.view?.job === j.id) {
    S.view = imageHistory()[0] ? { id: imageHistory()[0].id, idx: 0 } : null;
  }
  delete LOCALJ[j.id]; delete IMGJ[j.id];
  syncImageJobs(true);
}
function notify(entry) {
  if (document.hidden && "Notification" in window && Notification.permission === "granted") {
    new Notification("Tiiny Studio", { body: `${entry.mode === "edit" ? "Edit" : "Image"} ready in ${fmtSec(entry.seconds)}`, icon: "/images/" + entry.outputs[0] });
  }
}
function cancelQueued(jid) {
  if (S.view && S.view.job === jid) S.view = imageHistory()[0] ? { id: imageHistory()[0].id, idx: 0 } : null;
  cancelJob(jid);
}

/* ---------- status ---------- */
function renderStatus() {
  const dot = $("#statusDot");
  dot.className = "dot " + (S.deviceOk === false ? "err" : S.running ? "busy" : S.deviceOk ? "ok" : "");
  const q = S.queue.length;
  $("#queueMeta").textContent = S.running ? `Running${q ? ` · ${q} queued` : ""}` : S.deviceOk === false ? (S.keyRejected ? "API key rejected" : "Device offline") : "Device idle";
}

/* ---------- stage ---------- */
let tick = null;
function renderStage() {
  clearInterval(tick); tick = null;
  const stage = $("#stage"), head = $("#stageHead");
  stage.replaceChildren(); head.replaceChildren();

  const liveJob = S.running || S.queue[0];
  const viewingJob = S.view && S.view.job != null;
  if (liveJob && !viewingJob) {
    head.append(el("button", { class: "pill live", onclick: () => { S.view = { job: (S.running || S.queue[0]).jid }; renderStage(); } },
      el("span", { class: "dot busy" }), S.running ? "Generating… " : "Queued", el("span", { id: "liveTimer" })));
  }

  if (viewingJob) {
    const job = S.running && S.running.jid === S.view.job ? S.running : S.queue.find((j) => j.jid === S.view.job);
    if (job) return renderPending(stage, head, job);
    S.view = null;
  }

  let entry = S.view && S.history.find((e) => e.id === S.view.id);
  if (!entry && imageHistory()[0]) { entry = imageHistory()[0]; S.view = { id: entry.id, idx: 0 }; }
  if (!entry) return renderEmpty(stage);

  const idx = Math.min(S.view.idx || 0, entry.outputs.length - 1);
  const url = "/images/" + entry.outputs[idx];
  const p = entry.params;
  head.append(el("div", { class: "crumb" }, el("b", {}, entry.mode === "edit" ? "Edit" : entry.mode === "refs" ? "Created from references" : "Created"), " · " + timeAgo(entry.created)));
  head.append(el("div", { class: "spacer" }));
  const tb = el("div", { class: "toolbar" },
    el("button", { class: "tb primary", title: "Use this image as the source for an edit (E)", onclick: () => useAsSource(url) }, icon("i-brush"), el("span", { class: "lbl" }, "Edit this")),
    el("button", { class: "tb", title: "Turn this image into a 3D model", onclick: () => MeshWS.fromImage(url) }, icon("i-cube"), el("span", { class: "lbl" }, "Make 3D")),
    modelForTool("video") ? el("button", { class: "tb", title: "Start a video clip from this image", onclick: () => VideoWS.fromImage(url) }, icon("i-film"), el("span", { class: "lbl" }, "Animate")) : null,
    el("button", { class: "tb", title: "Describe this image with the chat model", disabled: !!S.describing[entry.outputs[idx]], onclick: () => describeImage(entry, idx) }, icon("i-text"), el("span", { class: "lbl" }, S.describing[entry.outputs[idx]] ? "Describing…" : "Describe")),
    entry.sources.length ? el("button", { class: "tb", "aria-pressed": S.compare, title: "Before / after (C)", onclick: () => { S.compare = !S.compare; renderStage(); } }, icon("i-cmp"), el("span", { class: "lbl" }, "Compare")) : null,
    el("button", { class: "tb", title: "Same settings, new seed (V)", onclick: () => rerun(entry, true) }, icon("i-redo"), el("span", { class: "lbl" }, "Vary")),
    el("button", { class: "tb", title: "Four new variations of this prompt, shown as a grid", "aria-label": "×4 new variations", onclick: () => variations(entry) }, el("span", { class: "lbl-x" }, "×4")),
    entry.outputs.length > 1 ? el("button", { class: "tb", "aria-pressed": !!S.grid, title: "Show all variations side by side", "aria-label": "Grid", onclick: () => { S.grid = !S.grid; renderStage(); } }, el("span", { class: "lbl-x" }, "▦"), el("span", { class: "lbl" }, "Grid")) : null,
    ocrModel() ? el("button", { class: "tb", title: "Read the text in this image (not saved)", "aria-label": "Text", onclick: () => ocrImage(url) }, el("span", { class: "lbl-x" }, "Aa"), el("span", { class: "lbl" }, "Text")) : null,
    el("button", { class: "tb", title: "Load these settings into the composer (R)", onclick: () => reuse(entry) }, icon("i-copy"), el("span", { class: "lbl" }, "Reuse")),
    el("a", { class: "tb", href: url, download: `tiiny-${entry.id}-${idx}.png`, title: "Download (D)" }, icon("i-down"), el("span", { class: "lbl" }, "Save")),
    el("button", { class: "tb danger", title: "Delete", "aria-label": "Delete", onclick: () => deleteImage(entry) }, icon("i-trash")));
  head.append(tb);

  if (S.grid && entry.outputs.length > 1) {  // all variations side by side; click one to open it
    stage.append(el("div", { class: "result-wrap" }, el("div", { class: "var-grid" + (entry.outputs.length > 2 ? " four" : "") },
      entry.outputs.map((o, i) => el("button", { class: "var-cell" + (i === idx ? " on" : ""), title: `Open variation ${i + 1}`, onclick: () => { S.view.idx = i; S.grid = false; renderStage(); } },
        el("img", { src: "/images/" + o, alt: `variation ${i + 1}` }), el("span", {}, i + 1)))),
      el("div", { class: "prompt-line" }, p.prompt)));
    return;
  }
  const canvas = el("div", { class: "canvas" + (S.view.fresh ? " fade-in" : "") });
  S.view.fresh = false;
  const img = el("img", { class: "main-img", src: url, alt: p.prompt });
  canvas.append(img);
  if (S.compare && entry.sources[0]) {
    const before = el("img", { class: "before", src: "/images/" + entry.sources[0], alt: "before" });
    const handle = el("div", { class: "cmp-handle", role: "slider", "aria-label": "Compare position", "aria-valuemin": "0", "aria-valuemax": "100", tabindex: "0" });
    canvas.append(before, handle, el("span", { class: "cmp-label", style: "left:10px" }, "Before"), el("span", { class: "cmp-label", style: "right:10px" }, "After"));
    const set = (f) => { f = Math.max(0, Math.min(1, f)); before.style.clipPath = `inset(0 ${100 - f * 100}% 0 0)`; handle.style.left = `calc(${f * 100}% - 12px)`; handle.dataset.f = f;
      const n = Math.round(f * 100); handle.setAttribute("aria-valuenow", n); handle.setAttribute("aria-valuetext", `${n}% before, ${100 - n}% after`); };
    set(.5);
    const move = (ev) => { const r = canvas.getBoundingClientRect(); set((ev.clientX - r.left) / r.width); };
    canvas.addEventListener("pointerdown", (ev) => { canvas.setPointerCapture(ev.pointerId); move(ev); canvas.onpointermove = move; });
    canvas.addEventListener("pointerup", () => { canvas.onpointermove = null; });
    handle.addEventListener("keydown", (ev) => { const f = Number(handle.dataset.f); if (ev.key === "ArrowLeft") { set(f - .05); ev.preventDefault(); ev.stopPropagation(); } if (ev.key === "ArrowRight") { set(f + .05); ev.preventDefault(); ev.stopPropagation(); } });
  }
  if (entry.outputs.length > 1) {
    canvas.append(el("div", { class: "variants" }, entry.outputs.map((o, i) =>
      el("button", { "aria-pressed": i === idx, title: `Variant ${i + 1}`, onclick: (ev) => { ev.stopPropagation(); S.view.idx = i; renderStage(); } }, el("img", { src: "/images/" + o, alt: "" })))));
  }
  const wrap = el("div", { class: "result-wrap" }, canvas,
    el("div", { class: "prompt-line", title: p.prompt }, p.prompt),
    el("div", { class: "meta-line" },
      el("span", {}, el("b", {}, (p.model || "").split("/").pop())),
      el("span", {}, p.size?.replace("x", " × ")),
      p.steps ? el("span", {}, `${p.steps} steps`) : null,
      p.guidance_scale ? el("span", {}, `cfg ${p.guidance_scale}`) : null,
      p.seed != null ? el("button", { class: "seed-copy", title: "Click to copy seed", onclick: () => { navigator.clipboard?.writeText(String(p.seed)); toast("Seed copied"); } }, `seed ${p.seed}`) : null,
      el("span", {}, fmtSec(entry.seconds))));
  const cap = S.captions[entry.outputs[idx]];
  if (cap) wrap.append(el("div", { class: "caption-box" }, el("p", {}, cap),
    el("div", { class: "row" }, el("small", {}, "Description"), el("span", { class: "spacer" }),
      el("button", { class: "linkish", onclick: () => { navigator.clipboard?.writeText(cap); toast("Copied"); } }, "Copy"),
      el("button", { class: "linkish", onclick: () => { setMode("create"); $("#prompt").value = cap; autoGrow(); renderComposer(); $("#prompt").focus(); } }, "Use as prompt"),
      el("button", { class: "linkish", "aria-label": "Dismiss", onclick: () => { delete S.captions[entry.outputs[idx]]; renderStage(); } }, "✕"))));
  const gate = S.describeGate === entry.outputs[idx] && modelGate("vision", "image-reading chat");
  wrap.append(el("div", { id: "describeSlot" }, gate ? el("div", { class: "assist-gate", "data-tool": "vision" }, gate) : null));
  stage.append(wrap);
  startLiveTimer();
}

function renderPending(stage, head, job) {
  const running = S.running === job;
  const pos = job.pos || S.queue.indexOf(job) + 1;
  head.append(el("div", { class: "crumb" }, el("b", {}, running ? (job.images.length ? "Editing" : "Generating") : `Queued #${pos}`), " · " + (job.params.model || "").split("/").pop()));
  const [w, h] = job.params.size.split("x").map(Number);
  const box = el("div", { class: "pending", style: `aspect-ratio:${w}/${h};width:min(100%, calc((100vh - 290px) * ${w / h}))` });
  if (job.images[0]) box.append(el("img", { class: "src-ghost", src: job.images[0], alt: "" }));
  const big = el("div", { class: "big" }, running ? "0s" : "Waiting");
  const sub = el("div", { class: "sub" }, running ? `about ${fmtSec(job.est)} · ${w}×${h}` : `starts after ${pos === 1 ? "the current job" : pos - 1 + " more"}`);
  const bar = el("i");
  box.append(el("div", { class: "pending-inner" }, big, sub, el("div", { class: "bar" }, bar), el("div", { class: "prompt-echo" }, job.params.prompt),
    running ? el("button", { class: "tb cancel", onclick: (e) => stopButton(e.currentTarget, job.jid) }, "Cancel")
      : el("button", { class: "tb cancel", onclick: () => cancelQueued(job.jid) }, "Remove from queue")));
  stage.append(box);
  if (running) {
    const upd = () => {
      const t = (Date.now() - job.started) / 1000;
      big.textContent = fmtSec(t);
      const f = t < job.est ? t / job.est * 0.92 : 0.92 + (1 - Math.exp(-(t - job.est) / job.est)) * 0.07;
      bar.style.width = (f * 100).toFixed(1) + "%";
      if (t > job.est * 1.1) sub.textContent = `taking longer than usual · ${w}×${h}`;
    };
    upd(); tick = setInterval(upd, 1000);
  }
}

function startLiveTimer() {
  const t = $("#liveTimer");
  if (!t || !S.running) return;
  const upd = () => { t.textContent = fmtSec((Date.now() - S.running.started) / 1000) + (S.queue.length ? ` · +${S.queue.length}` : ""); };
  upd(); tick = setInterval(() => S.running ? upd() : null, 1000);
}

function renderEmpty(stage) {
  stage.append(el("div", { class: "empty" },
    el("div", { class: "art" }, icon("i-spark")),
    el("h2", {}, "Make something, then keep shaping it"),
    el("p", {}, "Create an image from a prompt, then hit Edit this to refine it with plain-language instructions. Every result lands in the gallery below."),
    el("div", { class: "steps" }, el("span", {}, el("b", {}, "1"), "Describe"), el("span", {}, el("b", {}, "2"), "Generate"), el("span", {}, el("b", {}, "3"), "Edit this → refine"))));
}

/* ---------- strip ---------- */
function renderStrip() {
  const strip = $("#strip");
  const items = [];
  const pend = [...(S.running ? [S.running] : []), ...S.queue];
  for (const j of pend.slice().reverse()) {
    items.push(el("button", { class: "thumb queued" + (j === S.running ? " run" : ""), "aria-current": S.view?.job === j.jid, title: j.params.prompt, "aria-label": `${j === S.running ? "Running" : "Queued"}: ${j.params.prompt}`, onclick: () => { S.view = { job: j.jid }; renderStage(); renderStrip(); } }, j === S.running ? "running" : "queued"));
  }
  // extras.js (gallery tools) loads after this script; the very first render happens before it.
  const tools = typeof galleryFilter === "function";
  const shown = tools ? galleryFilter("image", imageHistory()) : imageHistory();
  if (tools) $("#galBar-image").replaceChildren(galleryBar("image", shown, renderStrip));
  for (const e of shown) {
    const dec = tools ? decorateThumb : (_ws, t) => t;
    items.push(dec("image", el("button", { class: "thumb", "aria-current": S.view?.id === e.id, title: e.params.prompt, "aria-label": e.params.prompt, onclick: () => { S.view = { id: e.id, idx: 0 }; S.compare = false; S.grid = false; renderStage(); renderStrip(); } },
      el("img", { src: "/images/" + e.outputs[0], alt: "", loading: "lazy" }),
      e.mode === "edit" ? el("span", { class: "badge" }, "EDIT") : e.mode === "refs" ? el("span", { class: "badge" }, "REFS") : null,
      e.outputs.length > 1 ? el("span", { class: "multi" }, "×" + e.outputs.length) : null), e, renderStrip, thumbDel(() => deleteImage(e))));
  }
  strip.replaceChildren(...(items.length ? items : [el("div", { class: "strip-empty" }, "Your images will collect here.")]));
  $("#galCount").textContent = imageHistory().length || "";
  const cur = strip.querySelector('[aria-current="true"]');
  // Scroll only the strip; scrollIntoView would also scroll the page sideways.
  if (cur) {
    const left = cur.offsetLeft - strip.offsetLeft;
    if (left < strip.scrollLeft || left + cur.offsetWidth > strip.scrollLeft + strip.clientWidth) strip.scrollLeft = Math.max(0, left - 8);
  }
}

/* ---------- chat helpers ---------- */
S.captions = {}; S.describing = {};
async function improvePrompt() {
  const p = $("#prompt"), before = p.value, edit = S.mode === "edit", btn = $("#improveBtn");
  if (!before.trim()) return toast(edit ? "Describe the change first" : "Type an idea first");
  btn.disabled = true; btn.lastChild.textContent = "Improving…";
  const text = await assist(edit ? "edit" : "prompt", { text: before.trim() }, $("#improveSlot"), edit ? "Improving instruction" : "Improving prompt");
  btn.disabled = false; btn.lastChild.textContent = "Improve prompt";
  if (text == null) return;
  p.value = text; autoGrow(); renderComposer();
  assistUndo($("#improveSlot"), "Rewritten", () => { p.value = before; autoGrow(); renderComposer(); });
}
async function describeImage(entry, idx) {
  const file = entry.outputs[idx];
  S.describeGate = helperGate("describe") ? file : null;  // renderStage shows the "load a model" card
  if (S.describeGate) return renderStage();
  S.describing[file] = true; renderStage();
  const text = await assist("describe", { file }, el("div"), "Describing image");
  delete S.describing[file];
  if (text != null) S.captions[file] = text;
  renderStage();
}

/* ---------- actions ---------- */
function reuse(entry, focus = true) {
  const p = entry.params;
  if (p.model && modelById(p.model)?.status === "running") selectModel(p.model);
  $("#prompt").value = p.prompt || ""; autoGrow();
  $("#negative").value = p.negative_prompt || "";
  $("#stepsAuto").checked = !p.steps; $("#steps").disabled = !p.steps; if (p.steps) $("#steps").value = p.steps;
  $("#cfgAuto").checked = !p.guidance_scale; $("#cfg").disabled = !p.guidance_scale; if (p.guidance_scale) $("#cfg").value = p.guidance_scale;
  syncSliders();
  $("#seed").value = p.seed ?? "";
  S.n = p.n || 1;
  const [w, h] = (p.size || "768x768").split("x").map(Number);
  const r = w / h, best = ASPECTS.filter((a) => a.w).reduce((b, a) => Math.abs(a.w / a.h - r) < Math.abs(b.w / b.h - r) ? a : b);
  S.res = [512, 768, 1024].reduce((b, v) => Math.abs(v * v - w * h) < Math.abs(b * b - w * h) ? v : b);
  if (modelSizeList(modelById(S.model))) S.res = Math.max(w, h);
  let ready = Promise.resolve();
  const sources = () => Promise.all(entry.sources.map(async (f) => { const b = await (await fetch("/images/" + f)).blob(); return readImage(new File([b], f, { type: b.type })); }));
  if (entry.mode === "edit" && entry.sources.length) {
    ready = sources().then((srcs) => { setMode("edit"); S.sources = srcs; S.aspect = "src"; renderComposer(); });
  } else {
    S.aspect = modelSizeList(modelById(S.model)) ? aspectKey(w, h) : best.id; setMode("create");
    S.sources = [];  // a plain create drops Create's references; "Created from references" brings its own back
    if (entry.mode === "refs" && entry.sources.length) ready = sources().then((srcs) => { S.sources = srcs; renderComposer(); });
    else renderComposer();
  }
  if (focus) { $("#prompt").focus(); toast("Settings loaded"); }
  return ready;
}
async function rerun(entry) {
  await reuse(entry, false);
  if (!S.seedLocked) $("#seed").value = "";
  submitImage();
}
async function variations(entry) {
  await reuse(entry, false);
  S.n = 4;
  if (!S.seedLocked) $("#seed").value = "";
  S.grid = true;
  submitImage();
  S.n = 1;
}
async function deleteImage(entry) {
  try { await api("/api/history/" + entry.id, { method: "DELETE" }); } catch (e) { return toast(e.message, "err"); }
  const ih = imageHistory(), i = ih.indexOf(entry);
  S.history.splice(S.history.indexOf(entry), 1);
  if (S.view?.id === entry.id) {  // only move the big display when its image is the one deleted
    const next = ih[i + 1] || ih[i - 1];
    S.view = next ? { id: next.id, idx: 0 } : null;
  }
  renderStrip(); renderStage();
}
function stepImage(d) {
  const ids = imageHistory().map((e) => e.id);
  let i = S.view?.id ? ids.indexOf(S.view.id) : -1;
  i = Math.max(0, Math.min(ids.length - 1, i + d));
  if (ids[i]) { S.view = { id: ids[i], idx: 0 }; S.compare = false; renderStage(); renderStrip(); }
}
function renderAll() { renderComposer(); renderStrip(); renderStage(); }

/* ---------- mark-area annotation ---------- */
const AN = { idx: 0, img: null, strokes: [], cur: null };
function openAnnot(i) {
  const s = S.sources[i]; if (!s) return;
  AN.idx = i; AN.strokes = []; AN.cur = null;
  AN.img = new Image();
  AN.img.onload = () => {
    const c = $("#annotCanvas"); c.width = AN.img.naturalWidth; c.height = AN.img.naturalHeight;
    $("#annotSize").value = Math.round(Math.max(c.width, c.height) / 40);
    drawAnnot(); openModal($("#annot"));
  };
  AN.img.src = s.orig || s.uri;
}
function closeAnnot() { closeModal($("#annot")); }
function drawAnnot() {
  const c = $("#annotCanvas"), g = c.getContext("2d");
  g.clearRect(0, 0, c.width, c.height);
  g.drawImage(AN.img, 0, 0);
  g.lineCap = g.lineJoin = "round"; g.strokeStyle = "rgba(255,32,32,.95)";
  for (const st of [...AN.strokes, AN.cur].filter(Boolean)) {
    g.lineWidth = st.size; g.beginPath();
    st.pts.forEach(([x, y], k) => (k ? g.lineTo(x, y) : g.moveTo(x, y)));
    if (st.pts.length === 1) g.lineTo(st.pts[0][0] + .1, st.pts[0][1]);
    g.stroke();
  }
}
(() => {
  const c = $("#annotCanvas");
  const pt = (e) => { const r = c.getBoundingClientRect(); return [(e.clientX - r.left) * c.width / r.width, (e.clientY - r.top) * c.height / r.height]; };
  c.addEventListener("pointerdown", (e) => { c.setPointerCapture(e.pointerId); AN.cur = { size: Number($("#annotSize").value), pts: [pt(e)] }; drawAnnot(); });
  c.addEventListener("pointermove", (e) => { if (AN.cur) { AN.cur.pts.push(pt(e)); drawAnnot(); } });
  const end = () => { if (AN.cur) { AN.strokes.push(AN.cur); AN.cur = null; drawAnnot(); } };
  c.addEventListener("pointerup", end); c.addEventListener("pointercancel", end);
  $("#annotUndo").onclick = () => { AN.strokes.pop(); drawAnnot(); };
  $("#annotClear").onclick = () => { AN.strokes = []; drawAnnot(); };
  $("#annotClose").onclick = $("#annotCancel").onclick = closeAnnot;
  $("#annotApply").onclick = () => {
    const s = S.sources[AN.idx];
    if (!AN.strokes.length) { if (s.orig) { s.uri = s.orig; delete s.orig; } }
    else { s.orig = s.orig || s.uri; s.uri = $("#annotCanvas").toDataURL("image/png"); }
    closeAnnot();
    const p = $("#prompt");
    if (s.orig && !p.value.trim()) { p.value = "In the area marked in red, "; autoGrow(); }
    p.focus(); renderComposer();
  };
})();

/* ---------- wiring ---------- */
function autoGrow() { const p = $("#prompt"); p.style.height = "auto"; p.style.height = Math.min(280, p.scrollHeight) + "px"; $("#charCount").textContent = p.value.length; }
function syncSliders() {
  $("#stepsOut").textContent = $("#stepsAuto").checked ? "—" : $("#steps").value;
  $("#cfgOut").textContent = $("#cfgAuto").checked ? "—" : $("#cfg").value;
}
const srcLabel = () => (S.mode === "text" ? "file to read" : S.mode === "create" ? "reference picture" : S.mode === "edit" && S.sources.length ? "reference image" : "source image");

$("#modeCreate").onclick = () => setMode("create");
$("#modeEdit").onclick = () => setMode("edit");
$("#modeText").onclick = () => { if (S.mode !== "text") { setMode("text"); S.sources = []; renderComposer(); } };
$("#improveBtn").onclick = improvePrompt;
$("#prompt").addEventListener("input", () => { autoGrow(); renderComposer(); });
$("#prompt").addEventListener("keydown", (e) => { if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); submitImage(); } });
$("#go").onclick = submitImage;
$("#fileInput").onchange = (e) => { addFiles(e.target.files, true); e.target.value = ""; };
// Resolution buttons are rebuilt per model/shape in renderComposer().
$("#nDec").onclick = () => { S.n = Math.max(1, S.n - 1); renderComposer(); };
$("#nInc").onclick = () => { S.n = Math.min(4, S.n + 1); renderComposer(); };
$("#negative").oninput = renderComposer;
$("#seed").oninput = (e) => { e.target.value = e.target.value.replace(/\D/g, ""); renderComposer(); };
$("#seedDice").onclick = () => { $("#seed").value = Math.floor(Math.random() * 2 ** 31); renderComposer(); };
$("#seedLock").onclick = (e) => {
  S.seedLocked = !S.seedLocked; e.currentTarget.setAttribute("aria-pressed", S.seedLocked);
  if (S.seedLocked && !$("#seed").value) { const last = imageHistory()[0]?.params.seed; $("#seed").value = last ?? Math.floor(Math.random() * 2 ** 31); }
  renderComposer();
};
for (const k of ["steps", "cfg"]) {
  $("#" + k).oninput = () => { syncSliders(); renderComposer(); };
  $("#" + k + "Auto").onchange = (e) => { $("#" + k).disabled = e.target.checked; syncSliders(); renderComposer(); };
}

// paste files anywhere: to the zone you're pointing at or last clicked, else the workspace's default
document.addEventListener("paste", (e) => {
  const all = [...(e.clipboardData?.files || [])];
  if (!all.length) return;
  if (pasteToZone(all)) return e.preventDefault();
  if (S.ws && S.ws !== "image") { const label = wsFiles(all); if (label) { e.preventDefault(); toast("Pasted as " + label); } return; }
  const files = S.mode === "text" ? all.slice(0, 1) : all.filter((f) => f.type.startsWith("image/"));
  if (!files.length) return;
  e.preventDefault();
  const full = S.mode !== "text" && S.sources.length >= Math.max(1, caps(modelById(S.model)).maxRefs);
  const label = srcLabel();
  addFiles(files);
  if (!full) toast("Pasted as " + label);
});
// drag files anywhere
let dragDepth = 0;
window.addEventListener("dragenter", (e) => { if ([...e.dataTransfer.types].includes("Files")) { dragDepth++; $("#dropveil").classList.add("show"); } });
window.addEventListener("dragleave", () => { if (--dragDepth <= 0) { dragDepth = 0; $("#dropveil").classList.remove("show"); } });
window.addEventListener("dragover", (e) => e.preventDefault());
window.addEventListener("drop", (e) => { e.preventDefault(); dragDepth = 0; $("#dropveil").classList.remove("show"); if (S.ws && S.ws !== "image") wsFiles([...e.dataTransfer.files]); else addFiles(e.dataTransfer.files); });

// keyboard
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") { if ($("#annot").classList.contains("show")) closeAnnot(); else if (S.drawer) closeDrawer(); return; }
  if (e.target.closest("input, textarea, select, [role=dialog]:not(#drawer)") || e.ctrlKey || e.metaKey || e.altKey) return;
  if (document.querySelector(".modal.show")) return;
  if (e.key === "m") { S.drawer ? closeDrawer() : openDrawer(); return; }
  if (S.drawer) return;  // the drawer is a dialog too: no image shortcuts while it is open
  if (S.ws && S.ws !== "image") return;
  const entry = S.view?.id && S.history.find((x) => x.id === S.view.id);
  if (e.key === "ArrowRight") { stepImage(1); e.preventDefault(); }
  else if (e.key === "ArrowLeft") { stepImage(-1); e.preventDefault(); }
  else if (!entry) return;
  else if (e.key === "e") useAsSource("/images/" + entry.outputs[S.view.idx || 0]);
  else if (e.key === "v") rerun(entry);
  else if (e.key === "r") reuse(entry);
  else if (e.key === "c" && entry.sources.length) { S.compare = !S.compare; renderStage(); }
  else if (e.key === "d") document.querySelector(".toolbar a[download]")?.click();
});

// ask for completion notifications on first run (generations are slow)
$("#go").addEventListener("click", () => { if ("Notification" in window && Notification.permission === "default") Notification.requestPermission(); }, { once: true });
// Jobs run on the server, so leaving or reloading the page doesn't lose them.

syncSliders(); autoGrow(); renderAll();
loadModels(); loadHistory();
