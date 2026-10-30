"use strict";
/* Gallery tools (search, smart search, favorites, select / bulk delete / zip), live device stats, OCR. */

/* ---------- gallery tools (one state per workspace) ---------- */
const GAL = {};
const galState = (ws) => (GAL[ws] ||= { q: "", fav: false, select: false, sel: new Set(), smart: null, smartQ: "", busy: false });
const entryText = (e) => [e.params?.prompt, e.params?.text, e.params?.caption, e.params?.lyrics, e.text].filter(Boolean).join(" ");

/** Filter (and, after a smart search, order) a workspace's results. */
function galleryFilter(ws, list) {
  const g = galState(ws);
  let out = g.fav ? list.filter((e) => e.fav) : list.slice();
  const q = g.q.trim().toLowerCase();
  if (q && g.smart && g.smartQ === g.q) {
    const best = g.smart[0]?.score || 0;
    const keep = new Map(g.smart.filter((r) => r.score >= Math.max(0.25, best - 0.15)).map((r, i) => [r.id, i]));
    out = out.filter((e) => keep.has(e.id)).sort((a, b) => keep.get(a.id) - keep.get(b.id));
  } else if (q) {
    out = out.filter((e) => entryText(e).toLowerCase().includes(q));
  }
  return out;
}

/* "Open folder" only works on the computer running the studio (not in Docker, not from another device):
   the server says so, and the button stays hidden otherwise. */
fetch("/api/settings").then((r) => r.json()).then((s) => { if (s.can_open_folder) document.body.dataset.openFolder = "1"; }).catch(() => {});
async function openResultsFolder() {
  try { const r = await fetch("/api/open-folder", { method: "POST" }); if (!r.ok) throw new Error((await r.json()).error?.message || r.statusText); }
  catch (e) { toast("Couldn't open the folder: " + e.message, "err"); }
}

/** Search / ★ / Select controls for a strip header. rerender() redraws that workspace's strip. */
function galleryBar(ws, visible, rerender) {
  const g = galState(ws);
  const input = el("input", { class: "gal-search", type: "search", placeholder: "Search…", value: g.q, "aria-label": "Search results" });
  input.oninput = () => { g.q = input.value; g.smart = null; g.focus = input.selectionStart; rerender(); };
  input.onblur = () => { g.focus = null; };
  if (g.focus != null) setTimeout(() => { input.focus(); input.setSelectionRange(g.focus, g.focus); }, 0);
  input.onkeydown = (e) => { if (e.key === "Enter" && g.q.trim()) smartSearch(ws, rerender); e.stopPropagation(); };
  const bar = el("span", { class: "gal-bar" }, input,
    el("button", { class: "gal-btn" + (g.smart && g.smartQ === g.q ? " on" : ""), disabled: !g.q.trim() || g.busy, title: "Search by meaning with the device's embedding model (Enter)", onclick: () => smartSearch(ws, rerender) }, g.busy ? "…" : "✨"),
    el("button", { class: "gal-btn" + (g.fav ? " on" : ""), "aria-pressed": g.fav, title: "Show favorites only", onclick: () => { g.fav = !g.fav; rerender(); } }, "★"),
    el("button", { class: "gal-btn" + (g.select ? " on" : ""), "aria-pressed": g.select, title: "Select several", onclick: () => { g.select = !g.select; g.sel.clear(); rerender(); } }, g.select ? "Done" : "Select"),
    el("button", { class: "gal-btn gal-open-folder", title: "Open the results folder", "aria-label": "Open the results folder", onclick: openResultsFolder }, icon("i-folder")));
  if (g.select) {
    const n = g.sel.size;
    const del = el("button", { class: "gal-btn danger", disabled: !n, title: "Delete the selected items" }, `Delete ${n || ""}`.trim());
    let armed = false;
    del.onclick = async () => {
      if (!armed) { armed = true; del.textContent = `Really delete ${n}?`; del.classList.add("armed"); setTimeout(() => { armed = false; del.textContent = `Delete ${n}`; del.classList.remove("armed"); }, 3000); return; }
      const ids = [...g.sel];
      try { await api("/api/history/delete", { method: "POST", body: JSON.stringify({ ids }) }); } catch (e) { return toast(e.message, "err"); }
      S.history = S.history.filter((e) => !g.sel.has(e.id));
      if (ws === "image" && g.sel.has(S.view?.id)) S.view = null;
      if (ws === "audio" && typeof A !== "undefined" && g.sel.has(A.view?.id)) A.view = null;
      if (ws === "mesh" && typeof M !== "undefined" && g.sel.has(M.view?.id)) M.view = null;
      toast(`Deleted ${ids.length}`); g.sel.clear(); g.select = false; rerender(); rerenderStage(ws);
    };
    bar.append(
      el("button", { class: "gal-btn", title: "Select all shown", onclick: () => { visible.forEach((e) => g.sel.add(e.id)); rerender(); } }, "All"),
      el("button", { class: "gal-btn", disabled: !n, title: "Download the selected items as a zip", onclick: () => exportZip([...g.sel]) }, "Zip"),
      del);
  }
  return bar;
}
function rerenderStage(ws) {
  if (ws === "image") renderStage();
  else if (ws === "audio" && AudioWS.mounted) AudioWS.renderStage();
  else if (ws === "mesh" && MeshWS.mounted) MeshWS.renderStage();
}

/** Star + selection overlay on a thumbnail; in select mode a click toggles selection. */
function decorateThumb(ws, thumb, e, rerender) {
  const g = galState(ws);
  const star = el("span", { class: "thumb-fav" + (e.fav ? " on" : ""), role: "button", tabindex: "0", title: e.fav ? "Unfavorite" : "Favorite" }, e.fav ? "★" : "☆");
  const toggleFav = async (ev) => {
    ev.stopPropagation(); ev.preventDefault();
    const fav = !e.fav;
    try { await api(`/api/history/${e.id}/fav`, { method: "POST", body: JSON.stringify({ fav }) }); e.fav = fav || undefined; rerender(); }
    catch (err) { toast(err.message, "err"); }
  };
  star.addEventListener("click", toggleFav);
  star.addEventListener("keydown", (ev) => { if (ev.key === "Enter" || ev.key === " ") toggleFav(ev); });
  thumb.append(star);
  if (g.select) {
    thumb.classList.add("selectable");
    if (g.sel.has(e.id)) thumb.classList.add("selected");
    thumb.append(el("span", { class: "thumb-check" }, g.sel.has(e.id) ? "✓" : ""));
    thumb.onclick = (ev) => { ev.preventDefault(); g.sel.has(e.id) ? g.sel.delete(e.id) : g.sel.add(e.id); rerender(); };
    thumb.querySelector(".thumb-del")?.remove();
  }
  return thumb;
}

async function smartSearch(ws, rerender) {
  const g = galState(ws);
  if (!g.q.trim()) return;
  g.busy = true; rerender();
  try {
    const r = await fetch("/api/search", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ query: g.q }) });
    const j = await r.json();
    if (r.status === 409 && j.need_model) {
      toast("Smart search uses the embedding model — loading it now (about 1% NPU)…");
      await loadModel(j.need_model);
      g.busy = false; rerender(); return;
    }
    if (!r.ok) throw new Error(j.error?.message || `HTTP ${r.status}`);
    g.smart = j.results; g.smartQ = g.q;
  } catch (e) { toast(e.message, "err"); }
  g.busy = false; rerender();
}

async function exportZip(ids) {
  try {
    const r = await fetch("/api/export", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ ids }) });
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    const blob = await r.blob();
    const a = el("a", { href: URL.createObjectURL(blob), download: (r.headers.get("Content-Disposition") || "").match(/filename="([^"]+)"/)?.[1] || "tiiny-studio.zip" });
    document.body.append(a); a.click(); a.remove();
    setTimeout(() => URL.revokeObjectURL(a.href), 5000);
  } catch (e) { toast("Zip failed: " + e.message, "err"); }
}

/* ---------- live device stats ---------- */
S.stats = null;
let statsTimer = null;
async function pollStats() {
  clearTimeout(statsTimer);
  try { S.stats = await api("/api/stats"); } catch { S.stats = null; }
  if (S.drawer) { const c = $("#statsCard"); if (c) c.replaceWith(statsCard()); }
  renderJobsPill();
  const busy = (S.jobs || []).some((j) => j.status === "running");
  if (S.drawer || busy) statsTimer = setTimeout(pollStats, 3000);
}
function statsCard() {
  const s = S.stats || {};
  const fmt = (v, unit, d = 0) => (typeof v === "number" ? `${v.toFixed(d)}${unit}` : "—");
  const bar = (v) => el("div", { class: "bar stat-bar" }, el("i", { style: `width:${Math.max(0, Math.min(100, v || 0))}%` }));
  const memPct = s.npu_mem_used_mb && s.npu_mem_total_mb ? s.npu_mem_used_mb / s.npu_mem_total_mb * 100 : null;
  return el("div", { class: "npu stats", id: "statsCard" },
    el("div", { class: "npu-top" }, el("span", {}, "Device activity"), el("span", {}, el("b", {}, fmt(s.npu_util, "%")), " NPU busy")),
    bar(s.npu_util),
    el("div", { class: "stat-grid" },
      el("div", {}, el("small", {}, "NPU memory"), el("b", {}, memPct != null ? `${Math.round(s.npu_mem_used_mb / 1024)} / ${Math.round(s.npu_mem_total_mb / 1024)} GB` : "—")),
      el("div", {}, el("small", {}, "NPU power"), el("b", {}, fmt(s.npu_power_w, " W", 1))),
      el("div", {}, el("small", {}, "Hottest sensor"), el("b", { class: s.npu_temp_c > 80 ? "hot" : "" }, fmt(s.npu_temp_c, " °C"))),
      el("div", {}, el("small", {}, "CPU / RAM"), el("b", {}, `${fmt(s.cpu_pct, "%")} / ${fmt(s.mem_pct, "%")}`))),
    (s.source || []).includes("dashboard") ? null : el("div", { class: "hint-line" }, "Power and temperature need the on-device dashboard (port 9109); NPU activity shows while a model is loaded."));
}

/* ---------- OCR ---------- */
TOOL_TYPES.ocr = "Image-to-Text";
const OCR_WAIT = new Map();
// Preferred order: PP-OCRv6 Small is fast, light and works on every link; GLM-OCR needs Wi-Fi/LAN.
const OCR_RANK = (m) => (/PP-OCR.*Small/i.test(m.id) ? 0 : /PP-OCR/i.test(m.id) ? 1 : 2);
const ocrModels = () => (S.allModels || []).filter((m) => m.type === "Image-to-Text" && !layoutOnly(m)).sort((a, b) => OCR_RANK(a) - OCR_RANK(b));
function ocrModel(id) {
  const all = ocrModels();
  return (id && all.find((m) => m.id === id)) || all.find((m) => m.status === "running") || all[0];
}
let ocrLastUrl = null;
async function ocrImage(url, modelId) {
  ocrLastUrl = url;
  let m = ocrModel(modelId || store.get("ocr.model", null));
  if (!m) return toast("No text-recognition (OCR) model is installed. Download one in the TiinyOS app.");
  if (wifiNote(m)) {  // GLM-OCR over USB: read with one that works on this link
    const alt = ocrModels().find((x) => !needsWifi(x));
    if (!alt) return toast(`${shortName(m.id)} needs a Wi-Fi/LAN connection: switch to the Tiiny's Wi-Fi address in Settings.`, "err");
    toast(`${shortName(m.id)} needs Wi-Fi/LAN, so this is read with ${shortName(alt.id)}.`);
    m = alt;
  }
  if (m.status !== "running") { S.ocrPending = url; toast(`Loading ${shortName(m.id)} first, then reading…`); return loadModel(m.id); }
  try {
    const uri = await blobToUri(await (await fetch(url)).blob());
    const job = await api("/api/jobs", { method: "POST", body: JSON.stringify({ kind: "ocr", model: m.id, image: uri, ephemeral: true, label: "Reading text" }) });
    S.jobs.push(job); OCR_WAIT.set(job.id, true); renderJobsPill(); pollJobs();
    toast("Reading the text…");
  } catch (e) { toast(e.message, "err"); }
}
/** Merge screen-wrapped OCR lines into paragraphs. A break stays after a blank line, before a list
    item, and after a line that ends a paragraph: noticeably shorter than the longest line and either
    ending a sentence or followed by a capital (a heading). Hyphenated words are rejoined. */
const CJK = /[　-鿿가-힯＀-￯]/;
const LIST_ITEM = /^\s*(\d{1,3}[.)]|[a-zA-Z][.)]|[-•*–·▪●◦])\s/;
function joinLines(text) {
  const lines = text.split("\n").map((l) => l.trimEnd());
  const max = Math.max(1, ...lines.map((l) => l.length));
  let out = "";
  lines.forEach((line, i) => {
    if (i === 0) { out = line; return; }
    const prev = lines[i - 1], cur = line.trimStart();
    // A short line followed by a capital is a label or heading, not wrapped prose (which runs ~35+ chars)
    const short = prev.length < max * 0.7 || prev.trim().length < 35;
    const keep = !prev.trim() || !cur || LIST_ITEM.test(line) || (short && (/[.!?:;"”)\]]$/.test(prev) || /^[A-Z0-9"“(]/.test(cur)));
    if (keep) out += "\n" + line;
    else if (/\p{L}-$/u.test(prev) && /^\p{Ll}/u.test(cur)) out = out.slice(0, -1) + cur;
    else out += (CJK.test(prev.slice(-1)) || CJK.test(cur[0]) ? "" : " ") + cur;
  });
  return out;
}
/* ---------- zip (stored, no compression: PNGs are already compressed) ---------- */
const CRC_TABLE = Array.from({ length: 256 }, (_, n) => { for (let k = 0; k < 8; k++) n = n & 1 ? 0xedb88320 ^ (n >>> 1) : n >>> 1; return n >>> 0; });
const crc32 = (b) => { let c = 0xffffffff; for (let i = 0; i < b.length; i++) c = CRC_TABLE[(c ^ b[i]) & 0xff] ^ (c >>> 8); return (c ^ 0xffffffff) >>> 0; };
const dataUriBytes = (uri) => Uint8Array.from(atob(uri.slice(uri.indexOf(",") + 1)), (c) => c.charCodeAt(0));
/** files: [{ name, data: Uint8Array }] -> a .zip Blob (UTF-8 names). */
function zipStore(files) {
  const enc = new TextEncoder(), chunks = [], central = [];
  let offset = 0;
  const now = new Date(), dosTime = (now.getHours() << 11) | (now.getMinutes() << 5) | (now.getSeconds() >> 1);
  const dosDate = ((now.getFullYear() - 1980) << 9) | ((now.getMonth() + 1) << 5) | now.getDate();
  for (const f of files) {
    const name = enc.encode(f.name), crc = crc32(f.data), size = f.data.length;
    const head = new DataView(new ArrayBuffer(30));
    [[0, 0x04034b50, 4], [4, 20, 2], [6, 0x0800, 2], [8, 0, 2], [10, dosTime, 2], [12, dosDate, 2], [14, crc, 4], [18, size, 4], [22, size, 4], [26, name.length, 2], [28, 0, 2]]
      .forEach(([o, v, n]) => (n === 4 ? head.setUint32(o, v, true) : head.setUint16(o, v, true)));
    chunks.push(head, name, f.data);
    const cd = new DataView(new ArrayBuffer(46));
    [[0, 0x02014b50, 4], [4, 20, 2], [6, 20, 2], [8, 0x0800, 2], [10, 0, 2], [12, dosTime, 2], [14, dosDate, 2], [16, crc, 4], [20, size, 4], [24, size, 4],
     [28, name.length, 2], [30, 0, 2], [32, 0, 2], [34, 0, 2], [36, 0, 2], [38, 0, 4], [42, offset, 4]]
      .forEach(([o, v, n]) => (n === 4 ? cd.setUint32(o, v, true) : cd.setUint16(o, v, true)));
    central.push(cd, name);
    offset += 30 + name.length + size;
  }
  const cdSize = central.reduce((s, c) => s + c.byteLength, 0);
  const end = new DataView(new ArrayBuffer(22));
  [[0, 0x06054b50, 4], [8, files.length, 2], [10, files.length, 2], [12, cdSize, 4], [16, offset, 4]]
    .forEach(([o, v, n]) => (n === 4 ? end.setUint32(o, v, true) : end.setUint16(o, v, true)));
  return new Blob([...chunks, ...central, end], { type: "application/zip" });
}

/** A download name from a document name: keeps spaces and accents, drops the extension and the characters
    Windows (\ / : * ? " < > |) or Obsidian links (# ^ [ ]) don't allow. */
const fileBase = (name) => name.replace(/\.\w{1,5}$/, "").replace(/[\\/:*?"<>|#^[\]\u0000-\u001f]+/g, " ").replace(/\s+/g, " ").replace(/[ .]+$/, "").trim() || "document";
/** A relative path for a Markdown link: spaces and parentheses encoded so the link survives any renderer. */
const mdPath = (p) => p.replace(/ /g, "%20").replace(/\(/g, "%28").replace(/\)/g, "%29");

/* ---------- Markdown ---------- */
/** Escape what would otherwise turn plain text into Markdown syntax (same rules as extract.py). */
const mdEscape = (s) => s.replace(/([\\`*[\]<])/g, "\\$1")
  .replace(/^(\s*)([#>+-]|\d+[.)])(?=\s|$)/gm, (m, sp, mk) => sp + (/\d/.test(mk[0]) ? mk.slice(0, -1) + "\\" + mk.slice(-1) : "\\" + mk));
/** Push Markdown headings down two levels (# → ###, at most ######), leaving code blocks alone. */
const demote = (md) => md.split(/(^```[\s\S]*?^```)/m).map((chunk, i) => (i % 2 ? chunk : chunk.replace(/^(#{1,6}) /gm, (m, h) => "#".repeat(Math.min(6, h.length + 2)) + " "))).join("");
/** OCR'd or plain text as Markdown: wrapped lines joined into paragraphs, bullets and numbered lines as lists. */
function plainToMd(text) {
  const out = [];
  let prevList = false;
  for (const raw of joinLines(text || "").split("\n")) {
    const line = raw.trim();
    if (!line) { prevList = false; continue; }
    const bullet = line.match(/^[-•*–·▪●◦]\s+(.*)$/), num = line.match(/^(\d{1,3})[.)]\s+(.*)$/);
    const item = bullet ? "- " + mdEscape(bullet[1]) : num ? `${num[1]}. ` + mdEscape(num[2]) : null;
    out.push((out.length && !(item && prevList) ? "\n" : "") + (item || mdEscape(line)));
    prevList = !!item;
  }
  return out.join("\n");
}
/** A PDF page's text layer as text and Markdown: lines whose font is clearly bigger than the page's body
    text become headings (×1.8 → #, ×1.45 → ##, ×1.2 → ###). */
function pdfPageText(items) {
  const lines = [];
  let cur = { s: "", h: 0 };
  for (const it of items) {
    if (it.str) { cur.s += it.str; cur.h = Math.max(cur.h, Math.abs(it.height || 0) || Math.hypot(it.transform[2], it.transform[3])); }
    if (it.hasEOL) { lines.push(cur); cur = { s: "", h: 0 }; }
  }
  if (cur.s) lines.push(cur);
  const weight = {};
  for (const l of lines) if (l.s.trim()) { const k = Math.round(l.h * 2) / 2; weight[k] = (weight[k] || 0) + l.s.trim().length; }
  const body = Number(Object.entries(weight).sort((a, b) => b[1] - a[1])[0]?.[0]) || 0;
  const md = [];
  let para = [], lastHead = null;
  const flush = () => { if (para.length) md.push(plainToMd(para.join("\n"))); para = []; };
  for (const l of lines) {
    const s = l.s.trim(), r = body ? l.h / body : 1;
    if (s && r >= 1.2 && s.length <= 120) {
      const level = r >= 1.8 ? 1 : r >= 1.45 ? 2 : 3;
      if (lastHead && lastHead.level === level && !para.length) { lastHead.text += " " + s; md[md.length - 1] = "#".repeat(level) + " " + mdEscape(lastHead.text); continue; }
      flush();
      lastHead = { level, text: s };
      md.push("#".repeat(level) + " " + mdEscape(s));
    } else { if (s) lastHead = null; para.push(l.s); }
  }
  flush();
  return { text: lines.map((l) => l.s).join("\n"), md: md.join("\n\n") };
}

/** The text result window, shared by single pictures and documents. Shows plain text or Markdown; Copy and
    Save follow the view. Returns controls to update it. */
function textDialog(title, { onClose, models, docName } = {}) {
  const box = el("textarea", { class: "ocr-text", readonly: true });
  const meta = el("small", { class: "ocr-meta" });
  const bar = el("div", { class: "ocr-progress", hidden: true }, el("i"));
  let parts = [], joined = store.get("ocr.join", false), view = store.get("ocr.view", "text"), closed = false;
  const sectionOf = (p) => p.label.replace(/, picture \d+$/, "");
  // Pages, slides and sheets get a heading each; pictures inside them are marked. Join lines works inside each part.
  const shownText = () => {
    const many = new Set(parts.map(sectionOf)).size > 1;
    let last = null;
    return parts.map((p) => {
      const sec = sectionOf(p), pic = p.label.match(/picture (\d+)$/);
      const head = (many && sec !== last ? `--- ${sec} ---\n` : "") + (pic ? `[Picture ${pic[1]}]\n` : "");
      last = sec;
      return head + (joined ? joinLines(p.text) : p.text);
    }).join("\n\n").trim();
  };
  // attach: file name prefix when saving with pictures; each picture is then embedded from attachments/
  const shownMd = (attach) => {
    const many = new Set(parts.map(sectionOf)).size > 1;
    let last = null;
    const body = parts.map((p) => {
      const sec = sectionOf(p), pic = p.label.match(/picture (\d+)$/);
      let md = p.md ?? plainToMd(p.text);
      if (many) md = demote(md);  // headings inside a page/slide/section sit below its ## heading
      const head = many && sec !== last ? `## ${mdEscape(sec)}\n\n` : "";
      last = sec;
      const img = pic && attach && p.img ? `![Picture ${pic[1]}](${mdPath(`attachments/${attach}-picture-${pic[1]}.png`)})\n\n` : "";
      return head + img + (pic ? `> **Picture ${pic[1]}**\n>\n` + md.split("\n").map((l) => (l ? "> " + l : ">")).join("\n") : md);
    }).join("\n\n").trim();
    return (docName && !body.startsWith("# ") ? `# ${mdEscape(docName)}\n\n` : "") + body;  // unless it has its own title
  };
  const shown = () => (view === "md" ? shownMd() : shownText());
  const show = () => {
    box.value = parts.length ? shown() : ""; box.placeholder = "(no text found)";
    join.setAttribute("aria-pressed", String(joined)); join.hidden = view === "md";  // Markdown always joins wrapped lines
    for (const b of seg.children) b.setAttribute("aria-pressed", String(b.dataset.v === view));
    const pics = parts.filter((p) => p.img).length;
    save.textContent = view === "md" ? (pics ? "Save .md + pictures" : "Save .md") : "Save .txt";
    save.title = view === "md" && pics ? `Download a .zip: the note plus an attachments folder with its ${pics} picture${pics > 1 ? "s" : ""}. Unzip it into your notes folder.`
      : "Download as a file (made in your browser; nothing is stored)";
    copy.textContent = view === "md" ? "Copy as Markdown" : "Copy";
  };
  const seg = el("div", { class: "seg small ocr-view", role: "group", "aria-label": "Show as" },
    [["text", "Text"], ["md", "Markdown"]].map(([v, label]) => el("button", { "data-v": v, onclick: () => { view = v; store.set("ocr.view", v); show(); } }, label)));
  const join = el("button", { class: "btn-sm toggle", title: "Merge lines wrapped by the screen or page into whole paragraphs",
    onclick: () => { joined = !joined; store.set("ocr.join", joined); show(); } }, "Join lines");
  const stop = el("button", { class: "btn-sm ghost", hidden: true, title: "Stop after the current page" }, "Stop");
  const save = el("button", { class: "btn-sm", onclick: () => {
    const md = view === "md", base = fileBase(docName || title);
    const pics = parts.filter((p) => p.img && /picture \d+$/.test(p.label));
    let blob, name;
    if (md && pics.length) {  // note + attachments/ in a zip, so the pictures show where they were
      const files = [{ name: `${base}.md`, data: new TextEncoder().encode(shownMd(base)) }];
      for (const p of pics) files.push({ name: `attachments/${base}-picture-${p.label.match(/picture (\d+)$/)[1]}.png`, data: dataUriBytes(p.img) });
      blob = zipStore(files); name = `${base}.zip`;
    } else { blob = new Blob([shown()], { type: md ? "text/markdown" : "text/plain" }); name = base + (md ? ".md" : ".txt"); }
    const a = el("a", { href: URL.createObjectURL(blob), download: name });
    a.click(); setTimeout(() => URL.revokeObjectURL(a.href), 5000);
  } });
  const copy = el("button", { class: "btn-sm accent", onclick: () => { navigator.clipboard?.writeText(shown()); toast("Copied"); } });
  const close = () => { closed = true; $("#settingsModal").classList.remove("show"); onClose?.(); };
  const card = el("div", { class: "modal-card set-card" },
    el("div", { class: "modal-head" }, el("h3", { class: "ocr-title" }, title), el("span", { class: "spacer" }), seg, el("button", { class: "icon-btn", title: "Close", onclick: close }, "✕")),
    el("div", { class: "set-body" }, bar, box,
      el("div", { class: "row set-actions" }, meta, el("span", { class: "spacer" }), models || null, stop, join, save, copy)));
  $("#settingsModal").replaceChildren(card);
  $("#settingsModal").dataset.locked = "";
  $("#settingsModal").classList.add("show");
  show();
  return {
    get closed() { return closed; },
    set(list) { parts = list; show(); box.scrollTop = box.scrollHeight; },
    meta(text) { meta.textContent = text; },
    progress(frac) { bar.hidden = frac == null; if (frac != null) bar.firstChild.style.width = Math.round(frac * 100) + "%"; },
    stoppable(fn) { stop.hidden = !fn; stop.onclick = fn; },
  };
}
const OCR_QUIET = new Set();  // document pages: read by readDocument, not shown one by one
function showOcrResult(j) {
  OCR_WAIT.delete(j.id);
  if (OCR_QUIET.delete(j.id) || j.status !== "done") return;
  const models = ocrModels().length > 1 ? el("select", { class: "input", style: "width:auto;padding:4px 8px", title: "Read again with another model",
    onchange: (e) => { store.set("ocr.model", e.target.value); $("#settingsModal").classList.remove("show"); ocrImage(ocrLastUrl, e.target.value); } },
    ocrModels().map((m) => el("option", { value: m.id, selected: m.id === j.model }, shortName(m.id) + (m.status === "running" ? "" : " (not loaded)") + (wifiNote(m) ? " · needs Wi-Fi" : "")))) : null;
  const d = textDialog("Text in this image", { models });
  d.set([{ label: "Image", text: j.entry?.text || "" }]);
  d.meta(`${(j.entry?.lines || []).length} lines · ${shortName(j.model)} · not saved`);
}

/* ---------- documents in Text mode ----------
   PDFs (pdf.js) and TIFF scans (UTIF) open in the browser: pages with a text layer are read directly, the
   rest are drawn and sent through OCR one page at a time. Office/OpenDocument, EPUB, RTF, HTML and text
   files are unpacked by the server, in memory, into text and pictures; the pictures get OCR'd. */
const DOC_MAX_PAGES = 50;
const DOC_ACCEPT = "image/*,.pdf,.tif,.tiff,.docx,.docm,.pptx,.ppsx,.xlsx,.xlsm,.odt,.odp,.ods,.epub,.rtf,.html,.htm,.txt,.md,.csv,.tsv,.json,.xml,.srt,.vtt,.doc,.xls,.ppt";
const isPdf = (f) => f.type === "application/pdf" || /\.pdf$/i.test(f.name || "");
const isTiff = (f) => f.type === "image/tiff" || /\.tiff?$/i.test(f.name || "");
const isHeic = (f) => /heic|heif/i.test(f.type) || /\.hei[cf]$/i.test(f.name || "");
/** A picture the browser can open by itself (everything but TIFF and iPhone HEIC). */
const browserImage = (f) => f.type.startsWith("image/") && !isTiff(f) && !isHeic(f);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const loadScript = (src) => new Promise((res, rej) => document.head.append(el("script", { src, onload: res, onerror: () => rej(new Error("Couldn't load " + src)) })));
let pdfjsLib = null, utifReady = null;
async function pdfLib() {
  if (!pdfjsLib) {
    pdfjsLib = await import("/static/vendor/pdfjs/pdf.min.mjs");
    pdfjsLib.GlobalWorkerOptions.workerSrc = "/static/vendor/pdfjs/pdf.worker.min.mjs";
  }
  return pdfjsLib;
}
// UTIF reads `self.pako` when it loads, so pako's inflate goes first.
const utif = () => (utifReady ||= loadScript("/static/vendor/pako_inflate.min.js").then(() => loadScript("/static/vendor/utif.js")));

function canvasUri(src, w, h, max = 3072) {
  const k = Math.min(1, max / Math.max(w, h));
  const c = Object.assign(document.createElement("canvas"), { width: Math.round(w * k), height: Math.round(h * k) });
  const g = c.getContext("2d");
  g.fillStyle = "#fff"; g.fillRect(0, 0, c.width, c.height);  // transparent PNGs read better on white
  g.drawImage(src, 0, 0, c.width, c.height);
  return c.toDataURL("image/png");
}
async function tiffCanvases(buf) {
  await utif();
  return UTIF.decode(buf).filter((ifd) => ifd.t256 && ifd.t257).map((ifd) => () => {
    UTIF.decodeImage(buf, ifd);
    const c = Object.assign(document.createElement("canvas"), { width: ifd.width, height: ifd.height });
    c.getContext("2d").putImageData(new ImageData(new Uint8ClampedArray(UTIF.toRGBA8(ifd)), ifd.width, ifd.height), 0, 0);
    return c;
  });
}
/** Any picture (data URI, any browser format or TIFF) as a white-backed PNG the OCR model takes. */
async function toOcrUri(uri) {
  if (uri.startsWith("data:image/tiff")) {
    const c = (await tiffCanvases(await (await fetch(uri)).arrayBuffer()))[0]?.();
    if (!c) throw new Error("Couldn't open a TIFF picture");
    return canvasUri(c, c.width, c.height);
  }
  const img = new Image();
  await new Promise((res, rej) => { img.onload = res; img.onerror = () => rej(new Error("Couldn't open a picture")); img.src = uri; });
  return canvasUri(img, img.naturalWidth, img.naturalHeight);
}

async function* pdfParts(file, run) {
  const lib = await pdfLib();
  let pdf;
  try {
    pdf = await lib.getDocument({ data: new Uint8Array(await file.arrayBuffer()), cMapUrl: "/static/vendor/pdfjs/cmaps/", cMapPacked: true, isEvalSupported: false }).promise;
  } catch (e) { throw new Error(e.name === "PasswordException" ? "This PDF is password-protected. Open it, print it to a new PDF without a password, and try that." : "Couldn't open this PDF: " + e.message); }
  try {
    run.total = Math.min(pdf.numPages, DOC_MAX_PAGES); run.skipped = pdf.numPages - run.total;
    for (let i = 1; i <= run.total && !run.stop; i++) {
      const page = await pdf.getPage(i);
      const tc = await page.getTextContent();
      const { text, md } = pdfPageText(tc.items);
      // A page with (almost) no text layer is a scan or a picture: draw it at ~2200 px and OCR it.
      if (text.replace(/\s/g, "").length >= 20) yield { label: `Page ${i}`, text, md };
      else yield { label: `Page ${i}`, image: async () => {
        const v1 = page.getViewport({ scale: 1 }), vp = page.getViewport({ scale: Math.min(4, 2200 / Math.max(v1.width, v1.height)) });
        const c = Object.assign(document.createElement("canvas"), { width: Math.round(vp.width), height: Math.round(vp.height) });
        const g = c.getContext("2d"); g.fillStyle = "#fff"; g.fillRect(0, 0, c.width, c.height);
        // "print" renders without requestAnimationFrame, which stalls while the tab is in the background
        await page.render({ canvasContext: g, viewport: vp, intent: "print" }).promise;
        return c.toDataURL("image/png");
      } };
      page.cleanup();
    }
  } finally { pdf.destroy(); }
}
async function* tiffParts(file, run) {
  const pages = await tiffCanvases(await file.arrayBuffer());
  if (!pages.length) throw new Error("Couldn't find any pages in this TIFF");
  run.total = Math.min(pages.length, DOC_MAX_PAGES); run.skipped = pages.length - run.total;
  for (let i = 0; i < run.total && !run.stop; i++) yield { label: `Page ${i + 1}`, image: async () => { const c = pages[i](); return canvasUri(c, c.width, c.height); } };
}
async function* serverParts(file, run) {
  const r = await fetch("/api/extract", { method: "POST", headers: { "Content-Type": "application/octet-stream", "X-Filename": encodeURIComponent(file.name || "file") }, body: file });
  const j = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(j.error?.message || `Couldn't read this file (${r.status})`);
  run.total = j.parts.length;
  for (const p of j.parts) yield p.image ? { label: p.label, image: () => toOcrUri(p.image) } : p;
}

/** Load the preferred OCR model if needed and wait for it (the Models panel shows progress). */
async function ensureOcrModel(run, d) {
  let m = ocrModel(store.get("ocr.model", null));
  if (!m) throw new Error("This part needs OCR, and no text-recognition model is installed. Download one in the TiinyOS app.");
  if (m.status === "running") return m.id;
  d.meta(`Loading ${shortName(m.id)} for the scanned parts…`);
  await loadModel(m.id);
  for (const t0 = Date.now(); Date.now() - t0 < 6 * 60e3 && !run.stop; await sleep(3000)) {
    await loadModels(true);
    m = ocrModel(m.id);
    if (m?.status === "running") return m.id;
    if (S.confirm?.id === m?.id) d.meta(`${shortName(m.id)} needs room: choose what to unload in the Models panel.`);
  }
  if (run.stop) return null;
  throw new Error(`${shortName(m.id)} didn't finish loading`);
}
async function ocrRun(uri, model, run) {
  const job = await api("/api/jobs", { method: "POST", body: JSON.stringify({ kind: "ocr", model, image: uri, ephemeral: true, label: "Reading a document" }) });
  run.job = job.id; OCR_QUIET.add(job.id);
  S.jobs.push(job); renderJobsPill();
  try {
    for (;;) {
      await sleep(700);
      const j = (await api("/api/jobs")).find((x) => x.id === job.id);
      if (!j || j.status === "cancelled") return null;
      if (j.status === "error") throw new Error(j.error || "OCR failed");
      if (j.status === "done") return j.entry?.text || "";
    }
  } finally { run.job = null; }
}

let DOC_RUN = null;
async function readDocument(file) {
  if (isHeic(file)) return toast("iPhone HEIC photos can't be opened here. Send it as JPEG, or on the iPhone set Settings → Camera → Formats → Most Compatible.", "err");
  if (DOC_RUN) return toast("Already reading a document. Stop it first.", "err");
  const run = DOC_RUN = { stop: false, job: null, total: 0, skipped: 0 };
  const halt = () => { run.stop = true; if (run.job) api(`/api/jobs/${run.job}/cancel`, { method: "POST" }).catch(() => {}); };
  const d = textDialog(file.name || "Pasted file", { onClose: halt, docName: file.name || "Document" });
  d.stoppable(halt); d.progress(0); d.meta("Opening…");
  const out = [];
  let direct = 0, ocred = 0, model = null, i = 0;
  try {
    const parts = isPdf(file) ? pdfParts(file, run) : isTiff(file) ? tiffParts(file, run) : serverParts(file, run);
    for await (const p of parts) {
      if (run.stop) break;
      i++;
      const where = run.total > 1 ? `${p.label} (${i} of ${run.total})` : p.label;
      if (p.text != null) { out.push({ label: p.label, text: p.text, md: p.md }); direct++; }
      else {
        model ||= await ensureOcrModel(run, d);
        if (run.stop) break;
        d.meta(`Reading ${where} with ${shortName(model)}…`);
        const uri = await p.image();
        const text = await ocrRun(uri, model, run);
        if (text == null) break;
        // pictures inside documents are kept (in memory) for the Markdown + pictures download; scanned pages aren't
        out.push({ label: p.label, text, img: /picture \d+$/.test(p.label) ? uri : undefined }); ocred++;
      }
      d.set(out); d.progress(run.total ? i / run.total : null);
      d.meta(`Read ${where}…`);
    }
    const bits = [`${out.length} ${out.length === 1 ? "part" : "parts"}`];
    if (direct && !ocred) bits.push("read directly, no OCR needed");
    else if (direct && ocred) bits.push(`${direct} read directly, ${ocred} by OCR`);
    else if (ocred) bits.push("by OCR");
    if (model) bits.push(shortName(model));
    if (run.stop && out.length < run.total) bits.unshift(`Stopped after ${out.length} of ${run.total}`);
    if (run.skipped > 0) bits.push(`only the first ${DOC_MAX_PAGES} pages (${run.skipped} more not read)`);
    bits.push("not saved");
    d.meta(bits.join(" · "));
    if (!out.length && !run.stop) d.meta("No text found · not saved");
  } catch (e) {
    d.meta(e.message); toast(e.message, "err");
  } finally {
    DOC_RUN = null; d.stoppable(null); d.progress(null);
    if (!d.closed) d.set(out);
  }
}
