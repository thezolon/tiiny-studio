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
async function chatSend() {
  const box = $("#chatInput"), text = box.value.trim();
  if (C.busy || (!text && !C.attach.length)) return;
  const model = runningChat();
  if (!model) { toast("No chat model is loaded. Load one from the Models panel.", "err"); return openDrawer(); }
  const conv = C.cur;
  conv.messages.push({ role: "user", text, attach: C.attach });
  if (!conv.title) conv.title = (text || C.attach[0]?.name || "Chat").slice(0, 60);
  C.attach = []; box.value = ""; autoGrowChat();
  const reply = { role: "assistant", text: "", reasoning: "", artifacts: [], model: model.id, pending: true };
  conv.messages.push(reply);
  C.busy = true; ChatWS.render();
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
    for (;;) {
      await sleep(350);
      let j;
      try { j = await api(`/api/jobs/${job.id}`); } catch { continue; }
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
      el("aside", { class: "chat-side" },
        el("button", { class: "btn-sm accent chat-new", onclick: () => { if (!C.busy) { C.cur = newChat(); C.attach = []; this.render(); $("#chatInput")?.focus(); } } }, "+ New chat"),
        el("div", { class: "chat-list", id: "chatList" }),
        el("div", { class: "chat-side-foot" },
          el("label", { class: "chat-opt", title: "Keep conversations in the studio's data folder (chats) so you can come back to them. Off: nothing is written to disk." },
            el("input", { type: "checkbox", checked: C.save, onchange: (e) => { C.save = e.target.checked; store.set("chat.save", C.save); if (C.save) chatPersist(); } }), "Save chats"))),
      el("section", { class: "chat-main" },
        el("div", { class: "chat-head", id: "chatHead" }),
        el("div", { class: "chat-scroll", id: "chatScroll" }, el("div", { class: "chat-msgs", id: "chatMsgs" })),
        el("div", { class: "chat-composer", id: "chatComposer" })));
    // drop or paste pictures and documents onto the composer (once: the composer element stays, its contents re-render)
    fileZone($("#chatComposer"), { key: "chat", accept: { test: () => true }, what: "file", label: "attachment", take: chatAttach });
    this.loadList();
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
      el("button", { class: "x", title: "Delete this chat (completely)", onclick: () => this.remove(c.id) }, "✕")))
      : [el("p", { class: "chat-empty-list" }, C.save ? "Saved chats show up here." : "Chats aren't being saved.")]));
  },
  async open(id) {
    if (C.busy) return toast("Wait for the reply to finish", "err");
    try { const c = await api(`/api/chats/${id}`); C.cur = { id: c.id, title: c.title, created: c.created, messages: c.messages || [] }; C.attach = []; this.render(); }
    catch (e) { toast(e.message, "err"); }
  },
  async remove(id) {
    if (!confirm("Delete this chat? It's removed completely (pictures it made stay in the gallery).")) return;
    try { await api(`/api/chats/${id}/delete`, { method: "POST", body: "{}" }); } catch (e) { return toast(e.message, "err"); }
    if (C.cur?.id === id && !C.busy) C.cur = newChat();
    this.loadList(); this.render();
  },
  render() {
    if (!this.mounted) return;
    const m = runningChat();
    $("#chatHead").replaceChildren(...[
      el("span", { class: "chat-title" }, C.cur.title || "New chat"), el("span", { class: "spacer" }),
      C.cur.messages.length && !C.busy ? el("button", { class: "btn-sm ghost", title: "Start a new chat", onclick: () => { C.cur = newChat(); C.attach = []; this.render(); } }, "+ New") : null,
      m ? el("span", { class: "chat-model", title: m.id }, shortName(m.id) + (seesImages(m) ? " · sees images" : ""))
        : el("button", { class: "btn-sm", onclick: openDrawer }, "Load a chat model")].filter(Boolean));
    const msgs = $("#chatMsgs");
    msgs.replaceChildren(...(C.cur.messages.length ? C.cur.messages.map((x) => this.bubble(x)) : [this.empty()]));
    this.renderComposer();
    this.renderList();
    $("#chatScroll").scrollTop = $("#chatScroll").scrollHeight;
  },
  renderLast() {
    const msgs = $("#chatMsgs"), last = C.cur.messages[C.cur.messages.length - 1];
    if (!msgs?.lastElementChild || !last) return;
    const sc = $("#chatScroll"), atBottom = sc.scrollHeight - sc.scrollTop - sc.clientHeight < 80;
    msgs.lastElementChild.replaceWith(this.bubble(last));
    if (atBottom) sc.scrollTop = sc.scrollHeight;
  },
  empty() {
    const m = runningChat();
    return el("div", { class: "chat-empty" }, el("h2", {}, "Chat with your Tiiny"),
      el("p", {}, m ? `Talking to ${shortName(m.id)}. Attach pictures or documents, or ask it to make something: an image, an edit, speech, a loop or a song. If a tool needs room on the device, the studio swaps models and swaps back.`
        : "Load a chat model from the Models panel to start."),
      el("div", { class: "chips" }, ["Make an image of a cosy reading nook on a rainy evening", "Summarise the attached document in five bullets", "Write a short poem and read it aloud", "Make a chill lo-fi piano loop"]
        .map((t) => el("button", { class: "chip", onclick: () => { $("#chatInput").value = t; autoGrowChat(); $("#chatInput").focus(); } }, t))));
  },
  bubble(x) {
    if (x.role === "user") {
      return el("div", { class: "msg user" },
        (x.attach || []).length ? el("div", { class: "msg-attach" }, x.attach.map((a) => a.kind === "image" ? el("img", { src: a.uri, alt: a.name }) : el("span", { class: "doc-chip", title: `${a.text.length.toLocaleString()} characters` }, "📄 " + a.name))) : null,
        x.text ? el("div", { class: "msg-text" }, x.text) : null);
    }
    const body = el("div", { class: "msg-md" });
    body.innerHTML = mdToHtml(x.text || "");
    return el("div", { class: "msg bot" + (x.error ? " err" : "") },
      // attachments that look like prompt injection: still sent, but the model was told not to obey them
      (x.warnings || []).length ? el("div", { class: "msg-warn", title: "The model was told to treat attachments as content only and never follow instructions inside them." },
        "⚠ Possible prompt injection in " + x.warnings.map((w) => `${w.source.replace(/^document: /, "")} (${w.reasons.join(", ").replace(/_/g, " ")})`).join("; ") + ". Check the reply with that in mind.") : null,
      x.reasoning ? el("details", { class: "msg-think", open: !!x.pending && !x.text }, el("summary", {}, x.pending && !x.text ? "Thinking…" : "Thoughts"), el("div", {}, x.reasoning)) : null,
      body,
      (x.artifacts || []).length ? el("div", { class: "msg-art" }, x.artifacts.map((a) => this.artifact(a))) : null,
      x.pending ? el("div", { class: "msg-status" }, el("span", { class: "dot busy" }), x.status || "Working…",
        el("button", { class: "btn-sm ghost", onclick: () => C.job && cancelJob(C.job) }, "Stop")) : null,
      x.error ? el("div", { class: "msg-error" }, x.error) : null,
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
    const prev = $("#chatInput")?.value || "";
    const input = el("textarea", { id: "chatInput", rows: "1", placeholder: C.busy ? "Wait for the reply…" : "Message your Tiiny — Enter to send, Shift+Enter for a new line",
      oninput: autoGrowChat, onkeydown: (e) => { if (e.key === "Enter" && !e.shiftKey && !e.isComposing) { e.preventDefault(); chatSend(); } } }, prev);
    const file = el("input", { type: "file", hidden: true, multiple: true, accept: DOC_ACCEPT, onchange: (e) => { chatAttach(e.target.files); e.target.value = ""; } });
    const toggle = (key, label, title) => el("button", { class: "btn-sm toggle", "aria-pressed": String(C[key]), title, onclick: () => { C[key] = !C[key]; store.set("chat." + key, C[key]); this.renderComposer(); } }, label);
    box.replaceChildren(...[
      C.attach.length ? el("div", { class: "chat-attach" }, C.attach.map((a, i) => el("span", { class: "att" },
        a.kind === "image" ? el("img", { src: a.uri, alt: "" }) : el("span", {}, "📄 " + a.name),
        el("button", { class: "x", title: "Remove", onclick: () => { C.attach.splice(i, 1); this.renderComposer(); } }, "✕")))) : null,
      el("div", { class: "chat-row" },
        el("button", { class: "icon-btn", title: "Attach pictures or documents (or paste / drop them)", onclick: () => file.click() }, icon("i-upload")), file,
        el("button", { class: "icon-btn" + (C.recorder ? " rec-on" : ""), title: C.recorder ? "Stop and transcribe" : "Speak your message", onclick: chatMic }, icon("i-mic")),
        input,
        el("button", { class: "go chat-send", disabled: C.busy, onclick: chatSend }, icon("i-spark"), "Send")),
      el("div", { class: "chat-opts" },
        toggle("thinking", "Thinking", "Let the model reason before answering (slower, often better)"),
        toggle("tools", "Can make things", "Let the model create and edit images, speak, and make loops and songs (models are swapped on the device when needed)"),
        el("span", { class: "spacer" }),
        el("small", { style: "color:var(--faint)" }, C.save ? "Saved in the data folder · delete removes it completely" : "Not saved"))].filter(Boolean));
    autoGrowChat();
  },
  files(files) { chatAttach(files); return "attachment"; },
  onModels() { if (this.mounted && S.ws === "chat") this.render(); },
};
