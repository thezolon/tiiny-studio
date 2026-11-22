"use strict";
/* Chat workspace: talk to the loaded model, attach images and documents, and let it make things with the
   studio's tools (images, edits, speech, loops, songs). The server swaps models on the device when a tool
   needs room and swaps back; this page streams the reply by polling the chat job. */

const C = {
  list: [], cur: null, job: null, attach: [], busy: false,
  thinking: store.get("chat.thinking", false), tools: store.get("chat.tools", true), save: store.get("chat.save", true),
  recorder: null,
};
const CHAT_TOOLS = ["create_image", "edit_image", "speak", "make_loop", "make_song"];
const newChat = () => ({ id: Math.random().toString(36).slice(2, 12), title: "", messages: [], created: Date.now() });

/* ---------- small, safe Markdown → HTML (text is escaped first; only http(s) links) ---------- */
const escHtml = (s) => s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
function mdInline(s) {
  return s.replace(/`([^`\n]+)`/g, "<code>$1</code>")
    .replace(/\*\*([^*\n]+)\*\*/g, "<b>$1</b>").replace(/(^|[\s(])\*([^*\n]+)\*(?=[\s).,!?:;]|$)/g, "$1<i>$2</i>")
    .replace(/\[([^\]\n]+)\]\((https?:\/\/[^\s)]+)\)/g, '<a href="$2" target="_blank" rel="noopener noreferrer">$1</a>');
}
function mdToHtml(src) {
  const out = [], lines = escHtml(src || "").split("\n");
  let i = 0;
  const table = (rows) => "<table>" + rows.map((r, k) => "<tr>" + r.map((c) => (k ? `<td>${mdInline(c)}</td>` : `<th>${mdInline(c)}</th>`)).join("") + "</tr>").join("") + "</table>";
  while (i < lines.length) {
    const l = lines[i];
    if (/^```/.test(l)) {
      const code = [];
      for (i++; i < lines.length && !/^```/.test(lines[i]); i++) code.push(lines[i]);
      out.push(`<pre><code>${code.join("\n")}</code></pre>`); i++; continue;
    }
    const h = l.match(/^(#{1,4})\s+(.*)$/);
    if (h) { out.push(`<h${h[1].length + 2}>${mdInline(h[2])}</h${h[1].length + 2}>`); i++; continue; }
    if (/^\s*\|.*\|\s*$/.test(l) && /^\s*\|[\s:|-]+\|\s*$/.test(lines[i + 1] || "")) {
      const rows = [l];
      for (i += 2; i < lines.length && /^\s*\|.*\|\s*$/.test(lines[i]); i++) rows.push(lines[i]);
      out.push(table(rows.map((r) => r.trim().slice(1, -1).split("|").map((c) => c.trim())))); continue;
    }
    if (/^\s*([-*•]|\d+[.)])\s+/.test(l)) {
      const ordered = /^\s*\d/.test(l), items = [];
      for (; i < lines.length && /^\s*([-*•]|\d+[.)])\s+/.test(lines[i]); i++) items.push(lines[i].replace(/^\s*([-*•]|\d+[.)])\s+/, ""));
      out.push(`<${ordered ? "ol" : "ul"}>${items.map((t) => `<li>${mdInline(t)}</li>`).join("")}</${ordered ? "ol" : "ul"}>`); continue;
    }
    if (/^&gt;\s?/.test(l)) {
      const q = [];
      for (; i < lines.length && /^&gt;\s?/.test(lines[i]); i++) q.push(lines[i].replace(/^&gt;\s?/, ""));
      out.push(`<blockquote>${mdInline(q.join("<br>"))}</blockquote>`); continue;
    }
    if (!l.trim()) { i++; continue; }
    const para = [];
    for (; i < lines.length && lines[i].trim() && !/^(```|#{1,4}\s|\s*([-*•]|\d+[.)])\s|&gt;)/.test(lines[i]); i++) para.push(lines[i]);
    out.push(`<p>${mdInline(para.join("<br>"))}</p>`);
  }
  return out.join("");
}
/* While a reply streams, keep its finished blocks in place and redraw only the unfinished tail: rebuilding a long reply
   full of tables on every update kept slower webviews (WebKitGTK on Linux) busy until the window stopped responding. */
function mdStream(body, text) {
  let cut = text.lastIndexOf("\n\n") + 1;  // blocks before a blank line are finished
  const fences = [...text.slice(0, cut).matchAll(/^```/gm)];
  if (fences.length % 2) cut = fences[fences.length - 1].index;  // not inside an open code block
  if (body._done === undefined || !text.startsWith(body._done) || cut < body._done.length) {  // new text, not a longer one
    body.replaceChildren(el("div", { class: "md-done" }), el("div", { class: "md-tail" }));
    body._done = "";
  }
  const [done, tail] = body.children;
  if (cut > body._done.length) { done.insertAdjacentHTML("beforeend", mdToHtml(text.slice(body._done.length, cut))); body._done = text.slice(0, cut); }
  tail.innerHTML = mdToHtml(text.slice(cut));
}

/* ---------- which models the tools use ---------- */
function chatToolModels() {
  const imgs = (S.models || []).filter((m) => !/Image-to-3D/.test(m.type));
  const create = imgs.find((m) => m.status === "running" && m.id === S.model) || imgs.find((m) => m.status === "running")
    || imgs.find((m) => m.id === store.get("model", null)) || imgs.slice().sort((a, b) => (a.npu_usage ?? 99) - (b.npu_usage ?? 99))[0];
  const edit = imgs.find((m) => m.status === "running" && canEditModel(m.id)) || imgs.find((m) => canEditModel(m.id));
  return { image: create?.id, edit: edit?.id, speech: modelForTool("speech")?.id, loop: modelForTool("loop")?.id, song: modelForTool("song")?.id };
}
const runningChat = () => (S.allModels || []).find((m) => m.status === "running" && CHAT_TYPES.includes(m.type));

/* ---------- conversation <-> API messages ---------- */
function apiMessages(conv, vision) {
  return conv.messages.filter((m) => !m.error).map((m) => {
    if (m.role === "assistant") return { role: "assistant", content: m.text || "(done)" };
    const docs = (m.attach || []).filter((a) => a.kind === "doc").map((a) => `\n\n--- Attached document: ${a.name} ---\n${a.text}`).join("");
    const imgs = (m.attach || []).filter((a) => a.kind === "image");
    const note = imgs.length && !vision ? `\n\n(${imgs.length} image${imgs.length > 1 ? "s" : ""} attached, but the loaded model can't see images.)` : "";
    const text = (m.text || "") + docs + note;
    if (!imgs.length || !vision) return { role: "user", content: text };
    return { role: "user", content: [{ type: "text", text: text || "See the attached image." }, ...imgs.map((a) => ({ type: "image_url", image_url: { url: a.uri } }))] };
  });
}
/** What tools made earlier in the chat, for the system prompt (kept out of the messages, where the model
    would imitate it instead of calling the tool). */
function madeSoFar(conv) {
  const made = conv.messages.flatMap((m) => m.artifacts || []);
  if (!made.length) return "";
  const what = { create_image: "image", edit_image: "edited image", speak: "speech", make_loop: "loop", make_song: "song preview" };
  return "Made earlier in this chat (already shown to the user): " + made.map((a, i) => `${i + 1}. ${what[a.tool] || a.kind}: ${a.prompt}`).join("; ")
    + ". The most recent image is the one edit_image changes.";
}
function latestImage(conv) {
  for (let i = conv.messages.length - 1; i >= 0; i--) {
    const m = conv.messages[i];
    const art = (m.artifacts || []).filter((a) => a.kind === "image" && a.outputs?.length).pop();
    if (art) return "/images/" + art.outputs[0];
    const at = (m.attach || []).filter((a) => a.kind === "image").pop();
    if (at) return at.uri;
  }
  return null;
}

/* ---------- attachments ---------- */
async function chatAttach(files) {
  for (const f of [...files].slice(0, 6)) {
    try {
      if (browserImage(f)) { const img = await readImage(f, 1536); C.attach.push({ kind: "image", uri: img.uri, name: f.name || "image" }); continue; }
      if (isHeic(f)) { toast("iPhone HEIC photos can't be opened in the browser. Send it as JPEG.", "err"); continue; }
      if (isTiff(f)) { toast("TIFF scans need OCR: read them in Image → Text mode, then paste the text here.", "err"); continue; }
      toast(`Reading ${f.name}…`);
      const run = { stop: false }, texts = [];
      let pictures = 0;
      const parts = isPdf(f) ? pdfParts(f, run) : serverParts(f, run);
      for await (const p of parts) { if (p.text != null) texts.push((run.total > 1 ? `[${p.label}]\n` : "") + p.text); else pictures++; }
      let text = texts.join("\n\n").trim();
      if (!text) { toast(`No text in ${f.name}${pictures ? " (it's scanned: read it in Image → Text mode)" : ""}`, "err"); continue; }
      if (text.length > 60000) { text = text.slice(0, 60000) + "\n[…cut: the document is longer than fits in one message]"; toast(`${f.name} is long: only the first part is attached`); }
      C.attach.push({ kind: "doc", name: f.name, text, pictures });
    } catch (e) { toast(e.message, "err"); }
  }
  ChatWS.renderComposer();
}

/* ---------- voice input (transcribed on the device) ---------- */
async function chatMic() {
  if (C.recorder) {
    const blob = await C.recorder.stop(); C.recorder = null; ChatWS.renderComposer();
    const asr = modelForTool("transcribe");
    if (!asr) return toast("No speech-recognition model is installed", "err");
    if (asr.status !== "running") { toast(`Loading ${shortName(asr.id)}; press the mic again in a moment`); return loadModel(asr.id); }
    try {
      const clip = await fileToWavClip(blob, 16000);
      toast("Transcribing…");
      const job = await submitJob("transcribe", { audio: clip.uri, ephemeral: true, name: "chat voice" }, "Voice message");
      for (;;) {
        await sleep(700);
        const j = await api(`/api/jobs/${job.id}`);
        if (j.status === "done") { const box = $("#chatInput"); box.value = (box.value ? box.value + " " : "") + (j.entry?.text || "").trim(); box.focus(); break; }
        if (j.status !== "queued" && j.status !== "running") { toast(j.error || "Couldn't transcribe", "err"); break; }
      }
    } catch (e) { toast(e.message, "err"); }
    return;
  }
  try { C.recorder = createRecorder(() => {}); await C.recorder.start(); ChatWS.renderComposer(); }
  catch (e) { C.recorder = null; toast("Microphone unavailable: " + e.message, "err"); }
}

/* ---------- sending and streaming ---------- */
/** again: { text, attach } of a message to send once more (Try again); what is typed in the box stays where it is. */
async function chatSend(again) {
  const box = $("#chatInput"), text = again ? again.text : box.value.trim(), attach = again ? again.attach : C.attach;
  if (C.busy || (!text && !attach.length)) return;
  const model = runningChat();
  if (!model) {
    const dp = deviceProblem();  // the real reason first: a locked or unreachable Tiiny isn't a missing model
    if (dp) return void toast(`${dp.title} ${dp.text}`, "err");
    toast("No chat model is loaded. Load one from the Models panel.", "err"); return openDrawer();
  }
  const conv = C.cur;
  conv.messages.push({ role: "user", text, attach });
  if (!conv.title) conv.title = (text || attach[0]?.name || "Chat").slice(0, 60);
  if (!again) { C.attach = []; box.value = ""; autoGrowChat(); }
  const reply = { role: "assistant", text: "", reasoning: "", artifacts: [], model: model.id, pending: true };
  conv.messages.push(reply);
  C.busy = true; ChatWS.render(true);
  const voices = await api("/api/voices").catch(() => []);
  const voice = voices.find((v) => v.id === store.get("chat.voice", null)) || voices[0];
  try {
    const job = await api("/api/jobs", { method: "POST", body: JSON.stringify({
      kind: "chat", label: "Chat", model: "default", messages: apiMessages({ messages: conv.messages.slice(0, -1) }, seesImages(model)),
      thinking: C.thinking, tools: C.tools ? CHAT_TOOLS : [], tool_models: chatToolModels(), voice_id: voice?.id,
      system: madeSoFar({ messages: conv.messages.slice(0, -1) }),
      last_image: latestImage({ messages: conv.messages.slice(0, -1) }), prefs: { size: "1024x1024" } }) });
    C.job = job.id; S.jobs.push(job); renderJobsPill();
    const t0 = Date.now();
    let fails = 0, failFrom = 0;
    for (;;) {
      await sleep(350);
      let j;
      try { j = await api(`/api/jobs/${job.id}`); fails = 0; }
      catch (e) {
        if (e.status === 404) { reply.error = "The studio restarted and this reply was lost. Send it again."; break; }  // the job is gone for good
        if (!fails++) failFrom = Date.now();
        if (Date.now() - failFrom > 60000) { reply.error = "Lost contact with the studio. Check that it is running, then send it again."; break; }
        continue;
      }
      reply.text = j.partial?.content || reply.text;
      reply.reasoning = j.partial?.reasoning || reply.reasoning;
      reply.status = j.status === "queued" ? (j.queue_pos ? `Waiting for the device (${j.queue_pos} ahead)` : "Waiting for the device") : (j.tool_message || j.message);
      reply.artifacts = j.artifacts || reply.artifacts;
      ChatWS.renderLast();
      if (j.warnings) reply.warnings = j.warnings;
      if (j.status === "done") { Object.assign(reply, { text: j.entry.text, reasoning: j.entry.reasoning, artifacts: j.entry.artifacts || [], model: j.entry.params?.model, seconds: Math.round((Date.now() - t0) / 1000) }); break; }
      if (j.status === "error" || j.status === "cancelled") { reply.error = j.status === "cancelled" ? "Stopped" : (j.error || "Something went wrong"); if (j.status === "cancelled" && reply.text) reply.error = null; break; }
    }
  } catch (e) { reply.error = e.message; }
  delete reply.pending; delete reply.status;
  C.busy = false; C.job = null;
  if (reply.artifacts.length) loadHistory();  // made things are in the gallery now
  ChatWS.render();
  chatPersist();
  pollJobs();
}
/** Try again after a failed or stopped reply: the same message goes out once more (what's typed in the box stays). */
function chatRetry(reply) {
  const msgs = C.cur.messages, i = msgs.indexOf(reply), user = msgs[i - 1];
  if (C.busy || i < 1 || user?.role !== "user") return;
  if (!runningChat()) { const dp = deviceProblem(); return void toast(dp ? `${dp.title} ${dp.text}` : "No chat model is loaded. Load one from the Models panel.", "err"); }
  msgs.splice(i - 1, 2);
  chatSend({ text: user.text, attach: user.attach || [] });
}
// thinking boxes the user opened or closed by hand stay that way while the reply streams (not saved with the chat)
const thinkOpen = new WeakMap();
async function chatPersist() {
  if (!C.save || !C.cur.messages.length) return;
  try { await api(`/api/chats/${C.cur.id}`, { method: "POST", body: JSON.stringify({ title: C.cur.title, created: C.cur.created, messages: C.cur.messages }) }); } catch (e) { toast(e.message, "err"); }
  ChatWS.loadList();
}
function autoGrowChat() { const b = $("#chatInput"); if (!b) return; b.style.height = "auto"; b.style.height = Math.min(240, b.scrollHeight) + "px"; }

/* ---------- workspace ---------- */
const ChatWS = {
  mount(root) {
    C.cur = newChat();
    root.replaceChildren(
      el("aside", { class: "chat-side", id: "chatSide", "aria-label": "Chats" },
        el("div", { class: "chat-side-top" },
          el("button", { class: "btn-sm accent chat-new", onclick: () => { if (C.busy) return toast("Wait for the reply to finish", "err"); C.cur = newChat(); C.attach = []; this.sheet(false); this.render(); $("#chatInput")?.focus(); } }, "+ New chat"),
          el("button", { class: "icon-btn sm chat-sheet-close", title: "Close", "aria-label": "Close the chat list", onclick: () => this.sheet(false) }, "✕")),
        el("div", { class: "chat-list", id: "chatList" }),
        el("div", { class: "chat-side-foot" },
          el("label", { class: "chat-opt", title: "Keep conversations in the studio's data folder (chats) so you can come back to them. Off: nothing is written to disk." },
            el("input", { type: "checkbox", checked: C.save, onchange: (e) => { C.save = e.target.checked; store.set("chat.save", C.save); if (C.save) chatPersist(); } }), "Save chats"))),
      el("div", { class: "chat-scrim", onclick: () => this.sheet(false) }),
      el("section", { class: "chat-main" },
        el("div", { class: "chat-head", id: "chatHead" }),
        el("div", { class: "chat-scroll", id: "chatScroll" }, el("div", { class: "chat-msgs", id: "chatMsgs" })),
        el("div", { class: "chat-composer", id: "chatComposer" })));
    // drop or paste pictures and documents onto the composer (once: the composer element stays, its contents re-render)
    fileZone($("#chatComposer"), { key: "chat", accept: { test: () => true }, what: "file", label: "attachment", take: chatAttach });
    this.loadList();
    document.addEventListener("keydown", (e) => { if (e.key === "Escape" && $("#chatSide")?.classList.contains("open")) this.sheet(false); });
  },
  /** Below 900 px the chat list is a sheet over the chat (opened by the Chats button in the head). */
  sheet(open) {
    const side = $("#chatSide");
    if (!side) return;
    side.classList.toggle("open", open);
    if (open) side.querySelector(".chat-new")?.focus();
    else if (side.contains(document.activeElement)) $("#chatSheetBtn")?.focus();
  },
  async loadList() {
    try { C.list = await api("/api/chats"); } catch { C.list = []; }
    this.renderList();
  },
  renderList() {
    const box = $("#chatList");
    if (!box) return;
    box.replaceChildren(...(C.list.length ? C.list.map((c) => el("div", { class: "chat-item" + (c.id === C.cur?.id ? " on" : "") },
      el("button", { class: "chat-item-open", title: c.title, onclick: () => this.open(c.id) }, el("b", {}, c.title), el("small", {}, new Date(c.updated * 1000).toLocaleString([], { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" }))),
      el("button", { class: "x", title: "Delete this chat (completely)", "aria-label": `Delete chat ${c.title || ""}`.trim(), onclick: () => this.remove(c.id) }, "✕")))
      : [el("p", { class: "chat-empty-list" }, C.save ? "Saved chats show up here." : "Chats aren't being saved.")]));
  },
  async open(id) {
    if (C.busy) return toast("Wait for the reply to finish", "err");
    try { const c = await api(`/api/chats/${id}`); C.cur = { id: c.id, title: c.title, created: c.created, messages: c.messages || [] }; C.attach = []; this.sheet(false); this.render(); }
    catch (e) { toast(e.message, "err"); }
  },
  async remove(id) {
    if (!confirm("Delete this chat? It's removed completely (pictures it made stay in the gallery).")) return;
    try { await api(`/api/chats/${id}/delete`, { method: "POST", body: "{}" }); } catch (e) { return toast(e.message, "err"); }
    if (C.cur?.id === id && !C.busy) C.cur = newChat();
    this.loadList(); this.render();
  },
  render(toBottom) {
    if (!this.mounted) return;
    // keep the reader's place when they scrolled up in the same chat (model refreshes re-render while a reply streams)
    const sc = $("#chatScroll"), top = sc.scrollTop, stay = !toBottom && this.shown === C.cur && sc.scrollHeight - top - sc.clientHeight >= 80;
    this.shown = C.cur;
    const m = runningChat();
    const dp = deviceProblem();
    $("#chatHead").replaceChildren(...[
      el("button", { class: "btn-sm ghost chat-sheet-btn", id: "chatSheetBtn", title: "Saved chats, new chat and the Save chats switch", onclick: () => this.sheet(true) }, "Chats"),
      el("span", { class: "chat-title" }, C.cur.title || "New chat"), el("span", { class: "spacer" }),
      C.cur.messages.length && !C.busy ? el("button", { class: "btn-sm ghost", title: "Start a new chat", onclick: () => { C.cur = newChat(); C.attach = []; this.render(); } }, "+ New") : null,
      m ? el("span", { class: "chat-model", title: m.id }, shortName(m.id) + (seesImages(m) ? " · sees images" : ""))
        : dp ? el("span", { class: "chat-model", title: `${dp.title} ${dp.text}` }, dp.short)  // the Tiiny can't be asked: no model to load yet
        : el("button", { class: "btn-sm", onclick: openDrawer }, "Load a chat model")].filter(Boolean));
    const msgs = $("#chatMsgs");
    msgs.replaceChildren(...(C.cur.messages.length ? C.cur.messages.map((x) => this.bubble(x)) : [this.empty()]));
    this.renderComposer();
    this.renderList();
    sc.scrollTop = stay ? top : sc.scrollHeight;
  },
  renderLast() {
    const msgs = $("#chatMsgs"), last = C.cur.messages[C.cur.messages.length - 1];
    if (!msgs?.lastElementChild || !last) return;
    const sig = [last.text, last.reasoning, last.status, (last.artifacts || []).length, (last.warnings || []).length].map((v) => v?.length ?? v).join("|");
    const old = msgs.lastElementChild;
    if (old._sig === sig) return;  // nothing new since the last poll
    const sc = $("#chatScroll"), atBottom = sc.scrollHeight - sc.scrollTop - sc.clientHeight < 80;
    // the thinking box is rebuilt on every update: keep where the user scrolled it (at its end: follow the new thinking)
    const think = old.querySelector(".msg-think div");
    const thinkTop = think?.scrollTop, thinkEnd = think && think.scrollHeight - think.scrollTop - think.clientHeight < 20;
    // a streaming reply keeps its text element: only what's new is drawn (mdStream); the rest of the bubble is rebuilt
    const body = last.pending && old.querySelector(":scope > .msg-md");
    const bubble = this.bubble(last, !!body);
    if (body) {
      mdStream(body, last.text || "");
      const kids = [...bubble.childNodes], i = kids.findIndex((n) => n.classList?.contains("md-slot"));
      for (const n of [...old.childNodes]) if (n !== body) n.remove();
      body.before(...kids.slice(0, i)); body.after(...kids.slice(i + 1));
      old.className = bubble.className;
    } else old.replaceWith(bubble);
    const cur = body ? old : bubble;
    cur._sig = sig;
    const nt = think && cur.querySelector(".msg-think div");
    if (nt) nt.scrollTop = thinkEnd ? nt.scrollHeight : thinkTop;
    if (atBottom) sc.scrollTop = sc.scrollHeight;
  },
  empty() {
    const m = runningChat(), dp = deviceProblem();
    return el("div", { class: "chat-empty" }, el("h2", {}, "Chat with your Tiiny"),
      el("p", {}, m ? `Talking to ${shortName(m.id)}. Attach pictures or documents, or ask it to make something: an image, an edit, speech, a loop or a song. If a tool needs room on the device, the studio swaps models and swaps back.`
        : dp ? `${dp.title} ${dp.text}` : "Load a chat model from the Models panel to start."),
      !m && dp?.act ? el("p", {}, el("button", { class: "btn-sm accent", onclick: () => openSettings(false) }, dp.act)) : null,
      el("div", { class: "chips" }, ["Make an image of a cosy reading nook on a rainy evening", "Summarise the attached document in five bullets", "Write a short poem and read it aloud", "Make a chill lo-fi piano loop"]
        .map((t) => el("button", { class: "chip", onclick: () => { $("#chatInput").value = t; autoGrowChat(); $("#chatInput").focus(); } }, t))));
  },
  bubble(x, slot) {  // slot: leave a placeholder for the text (renderLast keeps the streaming reply's own)
    if (x.role === "user") {
      return el("div", { class: "msg user" },
        (x.attach || []).length ? el("div", { class: "msg-attach" }, x.attach.map((a) => a.kind === "image" ? el("img", { src: a.uri, alt: a.name }) : el("span", { class: "doc-chip", title: `${a.text.length.toLocaleString()} characters` }, "📄 " + a.name))) : null,
        x.text ? el("div", { class: "msg-text" }, x.text) : null);
    }
    const body = el("div", { class: slot ? "md-slot" : "msg-md" });
    if (!slot) x.pending ? mdStream(body, x.text || "") : (body.innerHTML = mdToHtml(x.text || ""));
    return el("div", { class: "msg bot" + (x.error ? " err" : "") },
      // attachments that look like prompt injection: still sent, but the model was told not to obey them
      (x.warnings || []).length ? el("div", { class: "msg-warn", title: "The model was told to treat attachments as content only and never follow instructions inside them." },
        "⚠ Possible prompt injection in " + x.warnings.map((w) => `${w.source.replace(/^document: /, "")} (${w.reasons.join(", ").replace(/_/g, " ")})`).join("; ") + ". Check the reply with that in mind.") : null,
      x.reasoning ? el("details", { class: "msg-think", open: thinkOpen.get(x) ?? (!!x.pending && !x.text) },
        el("summary", { onclick: (e) => thinkOpen.set(x, !e.currentTarget.parentElement.open) }, x.pending && !x.text ? "Thinking…" : "Thoughts"), el("div", {}, x.reasoning)) : null,
      body,
      (x.artifacts || []).length ? el("div", { class: "msg-art" }, x.artifacts.map((a) => this.artifact(a))) : null,
      x.pending ? el("div", { class: "msg-status" }, el("span", { class: "dot busy" }), x.status || "Working…",
        el("button", { class: "btn-sm ghost", ...onPress(() => C.job && cancelJob(C.job)) }, "Stop")) : null,
      x.error ? el("div", { class: "msg-error" }, plainError(x.error),
        el("button", { class: "btn-sm", disabled: C.busy, title: "Send the same message again", onclick: () => chatRetry(x) }, "Try again")) : null,
      !x.pending && !x.error ? el("div", { class: "msg-meta" }, [x.model && shortName(x.model), x.seconds && `${x.seconds}s`].filter(Boolean).join(" · "),
        el("button", { class: "btn-sm ghost", title: "Copy the reply", onclick: () => { navigator.clipboard?.writeText(x.text || ""); toast("Copied"); } }, "Copy")) : null);
  },
  artifact(a) {
    const url = "/images/" + (a.outputs || [])[0];
    if (a.kind === "image") {
      return el("figure", { class: "art-img" }, el("img", { src: url, alt: a.prompt, loading: "lazy", title: "Open in the Image workspace",
        onclick: () => { S.view = { id: a.id, idx: 0 }; setWs("image"); if (typeof renderStrip === "function") { renderStrip(); renderStage(); } } }),
        el("figcaption", {}, a.prompt));
    }
    return el("div", { class: "art-audio" }, el("small", {}, { speak: "Speech", make_loop: "Loop", make_song: "Song preview" }[a.tool] || a.kind), makePlayer(url),
      a.tool === "make_song" ? el("small", { style: "color:var(--faint)" }, "Continue it to a full song in Audio") : null);
  },
  renderComposer() {
    const box = $("#chatComposer");
    if (!box) return;
    const old = $("#chatInput"), prev = old?.value || "";
    // model refreshes redraw the composer: keep the typing focus and the caret where they were
    const typing = old && document.activeElement === old ? [old.selectionStart, old.selectionEnd, old.selectionDirection] : null;
    const input = el("textarea", { id: "chatInput", rows: "1", placeholder: C.busy ? "Wait for the reply…" : "Message your Tiiny",
      // a touch keyboard has no Shift+Enter: there Enter starts a new line and the Send button sends
      oninput: autoGrowChat, onkeydown: (e) => { if (e.key === "Enter" && !e.shiftKey && !e.isComposing && !matchMedia("(hover: none)").matches) { e.preventDefault(); chatSend(); } } }, prev);
    const file = el("input", { type: "file", hidden: true, multiple: true, accept: DOC_ACCEPT, onchange: (e) => { chatAttach(e.target.files); e.target.value = ""; } });
    const toggle = (key, label, title) => el("button", { class: "btn-sm toggle", "aria-pressed": String(C[key]), title, onclick: () => { C[key] = !C[key]; store.set("chat." + key, C[key]); this.renderComposer(); } }, label);
    box.replaceChildren(...[
      C.attach.length ? el("div", { class: "chat-attach" }, C.attach.map((a, i) => el("span", { class: "att" },
        a.kind === "image" ? el("img", { src: a.uri, alt: "" }) : el("span", {}, "📄 " + a.name),
        el("button", { class: "x", title: "Remove", "aria-label": `Remove ${a.name || "attachment"}`, onclick: () => { C.attach.splice(i, 1); this.renderComposer(); } }, "✕")))) : null,
      el("div", { class: "chat-row" },
        el("button", { class: "icon-btn", title: "Attach pictures or documents (or paste / drop them)", onclick: () => file.click() }, icon("i-upload")), file,
        el("button", { class: "icon-btn" + (C.recorder ? " rec-on" : ""), title: C.recorder ? "Stop and transcribe" : "Speak your message", onclick: chatMic }, icon("i-mic")),
        input,
        el("button", { class: "go chat-send", disabled: C.busy, onclick: () => chatSend() }, icon("i-spark"), "Send")),
      el("div", { class: "chat-opts" },
        // the model's own thinking info decides: an on/off switch, a note that it always reasons, or nothing
        { switch: toggle("thinking", "Thinking", "Let the model reason before answering (slower, often better)"),
          always: el("span", { class: "btn-sm toggle on", title: `${shortName(runningChat()?.id || "")} always reasons before answering` }, "Always thinks") }[thinkingMode(runningChat())] || null,
        toggle("tools", "Can make things", "Let the model create and edit images, speak, and make loops and songs (models are swapped on the device when needed)"),
        el("span", { class: "spacer" }),
        el("small", { style: "color:var(--faint)" }, C.save ? "Saved in the data folder · delete removes it completely" : "Not saved"),
        el("small", { class: "key-hint", style: "color:var(--faint)" }, "Enter sends · Shift+Enter new line"))].filter(Boolean));
    autoGrowChat();
    if (typing) { input.focus({ preventScroll: true }); input.setSelectionRange(...typing); }
  },
  files(files) { chatAttach(files); return "attachment"; },
  onModels() { if (this.mounted && S.ws === "chat") this.render(); },
};
