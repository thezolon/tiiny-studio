"use strict";
/* Audio workspace: voice-clone speech, music loops, full songs, transcription. */

const LANGS = [["", "Auto"], ["en", "English"], ["zh", "Chinese"], ["ja", "Japanese"], ["ko", "Korean"], ["german", "German"],
  ["french", "French"], ["spanish", "Spanish"], ["italian", "Italian"], ["pt", "Portuguese"], ["russian", "Russian"],
  ["beijing_dialect", "Beijing dialect"], ["sichuan_dialect", "Sichuan dialect"]];
const ASR_LANGS = [["", "Auto-detect"], ["en", "English"], ["zh", "Chinese"], ["ja", "Japanese"], ["ko", "Korean"], ["de", "German"],
  ["fr", "French"], ["es", "Spanish"], ["it", "Italian"], ["pt", "Portuguese"], ["ru", "Russian"], ["ar", "Arabic"], ["hi", "Hindi"],
  ["vi", "Vietnamese"], ["th", "Thai"], ["id", "Indonesian"]];
const STYLE_CHIPS = ["Calm and warm", "Excited and upbeat", "Whispering", "Like a news anchor", "Slow and clear", "Sad, quiet"];
const FAMILIES = {
  Synth: ["Wavetable Synth", "Supersaw", "FM Synth", "Analog Pad", "Pluck Synth", "Lead Synth"],
  Keys: ["Grand Piano", "Electric Piano", "Rhodes", "Church Organ", "Harpsichord"],
  Bass: ["Sub Bass", "Reese Bass", "Synth Bass", "Electric Bass"],
  Strings: ["Violin", "Cello", "String Ensemble"],
  Mallets: ["Vibraphone", "Marimba", "Glockenspiel", "Kalimba"],
  Winds: ["Flute", "Ocarina", "Clarinet", "Pan Flute"],
  Guitar: ["Acoustic Guitar", "Electric Guitar", "Clean Guitar"],
  Brass: ["Trumpet", "French Horn", "Brass Section"],
  Vocals: ["Choir", "Vocal Chops"],
  Plucked: ["Harp", "Koto", "Pizzicato"],
};
const CHARACTER = ["warm", "bright", "dark", "airy", "gritty", "wide", "punchy", "soft", "glassy", "metallic"];
const FX = ["reverb", "delay", "chorus", "distortion", "sidechain", "lo-fi"];
const PHRASE = ["chord progression", "melody", "arp", "bassline", "rising", "complex", "simple"];
const KEYS = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"].flatMap((k) => [`${k} major`, `${k} minor`]);
const SONG_CHIPS = ["pop", "rock", "lofi", "edm", "jazz", "folk", "cinematic", "hip hop", "male vocals", "female vocals", "upbeat", "melancholic", "chill", "energetic", "piano", "acoustic guitar", "synth"];
const LYRICS_TEMPLATE = "[verse]\nCity lights are fading slow\nI keep walking where the rivers go\n\n[chorus]\nHold on, hold on to the night\nWe're burning like a paper kite\n\n[verse]\nEvery step a little song\nEvery mile I'm getting strong\n\n[chorus]\nHold on, hold on to the night\nWe're burning like a paper kite";
// SongGeneration's reference-audio conditioning fails on TiinyOS 0.1.35 ("xh2a backend fail" for every
// clip format), so the style-reference controls stay hidden until firmware fixes it.
const SONG_STYLE_REF = false;
// Typed text, lyrics and style stay in this page's memory only — never in browser storage.
for (const k of ["audio.text", "audio.caption", "audio.lyrics"]) { try { localStorage.removeItem("tiiny." + k); } catch {} }
const AUDIO_KINDS = new Set(["speech", "loop", "song", "transcript"]);
const TOOLS = [
  { id: "speech", label: "Voice", icon: "i-voice", what: "text-to-speech" },
  { id: "loop", label: "Loop", icon: "i-wave", what: "loop" },
  { id: "song", label: "Song", icon: "i-music", what: "song" },
  { id: "transcribe", label: "Transcribe", icon: "i-text", what: "speech-recognition" },
];

const A = {
  tool: store.get("audio.tool", "speech"), view: null, voices: [],
  speech: { voiceId: store.get("audio.voice", null), clip: null, clipText: "", clipName: "", text: "", language: store.get("audio.lang", ""), instruct: "" },
  loop: { family: "Synth", role: "Wavetable Synth", desc: "", character: new Set(["warm"]), fx: new Set(), phrase: new Set(["chord progression"]), bpm: 120, bars: 4, key: "A minor", custom: null, steps: 64, cfg: 6, seed: "" },
  song: { type: "mixed", caption: "", lyrics: "", seconds: 60, ref: null, seed: "", steps: "", temperature: "", top_k: "" },
  transcribe: { clip: null, language: "" },
  recorder: null, recFor: null,
  help: null, helpBusy: null,  // chat helper state: { key, gate } | { key, what, undo }
};

const AudioWS = {
  mounted: false,
  mount(root) {
    root.replaceChildren(
      el("aside", { class: "composer" },
        el("div", { class: "composer-scroll" },
          el("div", { class: "seg tool-seg", role: "group", "aria-label": "Audio tool", id: "aTools" }),
          el("div", { id: "aGate" }),
          el("div", { id: "aPanel" })),
        el("div", { class: "composer-foot" },
          el("button", { class: "go", id: "aGo" }, icon("i-spark"), el("span", { id: "aGoLabel" }, "Speak"), el("span", { class: "est", id: "aGoEst" })),
          el("div", { class: "foot-meta" }, el("span", { id: "aMeta" }, ""), el("span", {}, el("kbd", {}, "Ctrl"), " ", el("kbd", {}, "Enter"))))),
      el("section", { class: "stage-col" },
        el("div", { class: "stage-head", id: "aHead" }),
        el("div", { class: "stage", id: "aStage" }),
        el("div", { class: "strip-wrap" },
          el("div", { class: "strip-head" }, "Audio library ", el("span", { class: "count", id: "aCount" }), el("span", { id: "galBar-audio" }), el("span", { class: "spacer" }), el("span", { class: "hint", style: "color:var(--faint);font-weight:450" }, el("kbd", {}, "Space"), " play/pause")),
          el("div", { class: "strip", id: "aStrip" }))));
    $("#aGo").onclick = () => this.submit();
    root.addEventListener("keydown", (e) => { if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); this.submit(); } });
    this.loadVoices();
  },
  async loadVoices() { try { A.voices = await api("/api/voices"); } catch { A.voices = []; } if (!A.voices.some((v) => v.id === A.speech.voiceId)) A.speech.voiceId = A.voices[0]?.id || null; this.render(); },

  render() {
    if (!this.mounted) return;
    $("#aTools").replaceChildren(...TOOLS.map((t) => el("button", { "aria-pressed": A.tool === t.id, onclick: () => { A.tool = t.id; store.set("audio.tool", t.id); this.render(); } }, icon(t.icon), t.label)));
    this.renderGate();
    $("#aPanel").replaceChildren(...{ speech: () => this.speechPanel(), loop: () => this.loopPanel(), song: () => this.songPanel(), transcribe: () => this.transcribePanel() }[A.tool]());
    this.renderFoot(); this.renderStage(); this.renderStrip();
  },
  renderGate() {
    const t = TOOLS.find((x) => x.id === A.tool);
    const g = modelGate(A.tool, t.what);
    $("#aGate").replaceChildren(...(g ? [g] : []));
    this.renderFoot();
  },
  renderFoot() {
    const m = modelForTool(A.tool), ready = m?.status === "running";
    const p = this.payload(true);
    const labels = { speech: "Speak", loop: "Make loop", song: "Compose", transcribe: "Transcribe" };
    $("#aGoLabel").textContent = labels[A.tool];
    $("#aGoEst").textContent = p.ok ? "~" + fmtSec(p.est) : "";
    $("#aGo").disabled = !ready || !p.ok;
    $("#aGo").title = !ready ? "Load the model first" : p.why || "";
    $("#aMeta").textContent = m ? `${shortName(m.id)}${ready ? "" : " · not loaded"}` : "";
  },

  /* ---------- Voice ---------- */
  speechPanel() {
    const sp = A.speech;
    const voices = el("div", { class: "voice-list" },
      ...A.voices.map((v) => el("div", { class: "voice-card", "aria-pressed": sp.voiceId === v.id && !sp.clip, onclick: () => { sp.voiceId = v.id; sp.clip = null; store.set("audio.voice", v.id); this.render(); } },
        el("button", { class: "vplay", title: "Preview", onclick: (e) => { e.stopPropagation(); const a = new Audio("/images/" + v.file); if (currentAudio) currentAudio.pause(); currentAudio = a; a.play(); } }, icon("i-play")),
        el("span", { class: "vname" }, v.name),
        el("button", { class: "vdel", title: "Delete voice", onclick: async (e) => { e.stopPropagation(); if (!confirm(`Delete voice “${v.name}”?`)) return; await api("/api/voices/" + v.id, { method: "DELETE" }); this.loadVoices(); } }, "✕"))),
      el("button", { class: "voice-card add", "aria-pressed": !!sp.clip || !A.voices.length, onclick: () => { sp.clip = sp.clip || { pending: true }; this.render(); } }, "+ New voice"));
    const out = [el("div", { class: "field" }, el("div", { class: "label" }, "Voice", el("span", { class: "hint" }, "cloned from a 3–20 s clip")), voices)];
    if (sp.clip || !A.voices.length) out.push(this.clipEditor());
    out.push(el("div", { class: "field" },
      el("div", { class: "label" }, "What to say", el("span", { class: "hint" }, `${sp.text.length} chars`)),
      el("div", { class: "prompt-wrap" }, el("textarea", { rows: "6", placeholder: "Type what the voice should say…", oninput: (e) => { sp.text = e.target.value; e.target.closest(".field").querySelector(".hint").textContent = `${sp.text.length} chars`; this.renderFoot(); } }, sp.text))));
    const style = el("input", { class: "input", placeholder: "e.g. calm and warm, a little slower", value: sp.instruct, oninput: (e) => { sp.instruct = e.target.value; } });
    out.push(el("div", { class: "field" }, el("div", { class: "label" }, "Delivery", el("span", { class: "hint" }, "optional style instruction")), style,
      el("div", { class: "chips" }, STYLE_CHIPS.map((c) => el("button", { class: "chip", onclick: () => { sp.instruct = c; style.value = c; } }, c)))));
    out.push(el("div", { class: "field" }, el("div", { class: "label" }, "Language"),
      el("select", { class: "input", onchange: (e) => { sp.language = e.target.value; store.set("audio.lang", sp.language); } }, LANGS.map(([v, n]) => el("option", { value: v, selected: v === sp.language }, n)))));
    return out;
  },
  clipEditor() {
    const sp = A.speech, c = sp.clip && !sp.clip.pending ? sp.clip : null;
    const box = el("div", { class: "clip-editor" });
    box.append(el("div", { class: "label" }, "New voice from a clip", el("span", { class: "hint" }, "record, drop or paste audio")));
    fileZone(box, { key: "voice", accept: /^(audio|video)\//, what: "audio", label: "voice sample", take: (f) => this.takeClip("speech", f[0]) });
    box.append(this.captureRow("speech", c));
    if (c) {
      box.append(el("textarea", { class: "input", rows: "2", placeholder: "Exactly what's said in the clip (makes the clone much better)", oninput: (e) => { sp.clipText = e.target.value; } }, sp.clipText));
      const asr = modelForTool("transcribe");
      box.append(el("div", { class: "row", style: "margin-top:8px;flex-wrap:wrap" },
        el("button", { class: "btn-sm", disabled: asr?.status !== "running", title: asr?.status === "running" ? "Fill the transcript with the speech model" : "Load the ASR model to auto-transcribe", onclick: () => this.autoTranscribe() }, "✎ Auto-transcribe"),
        el("input", { class: "input grow", placeholder: "Voice name", value: sp.clipName, oninput: (e) => { sp.clipName = e.target.value; } }),
        el("button", { class: "btn-sm accent", onclick: () => this.saveVoice() }, "Save voice")));
    }
    return box;
  },
  /** Record/upload control shared by tools; target is where the clip goes. */
  captureRow(target, clip) {
    const recording = A.recorder?.active && A.recFor === target;
    const meter = el("i", { class: "level" });
    const rec = el("button", { class: "btn-sm rec" + (recording ? " on" : ""), onclick: () => this.toggleRecord(target, meter) }, icon("i-mic"), recording ? " Stop" : " Record");
    const up = el("button", { class: "btn-sm", onclick: () => { const i = el("input", { type: "file", accept: "audio/*" }); i.onchange = () => i.files[0] && this.takeClip(target, i.files[0]); i.click(); } }, "Upload…");
    const row = el("div", { class: "capture" }, rec, up, recording ? el("span", { class: "meter" }, meter) : null);
    if (clip?.uri) row.append(el("div", { class: "clip-preview" }, makePlayer(clip.uri), el("small", {}, `${clip.duration.toFixed(1)} s${clip.trimmed ? " (trimmed)" : ""}`)));
    return row;
  },
  async toggleRecord(target, meter) {
    if (A.recorder?.active) {
      const blob = await A.recorder.stop(); A.recorder = null;
      return this.takeClip(target, blob, "recording");
    }
    try {
      A.recorder = createRecorder((lv) => { meter.style.width = Math.round(lv * 100) + "%"; });
      A.recFor = target;
      await A.recorder.start();
      this.render();
    } catch (e) { A.recorder = null; toast("Microphone unavailable: " + e.message, "err"); }
  },
  async takeClip(target, blob, name) {
    try {
      const rate = target === "song" ? 48000 : target === "transcribe" ? 16000 : 24000;
      const maxSec = target === "speech" ? 20 : target === "song" ? 30 : 0;
      const clip = await fileToWavClip(blob, rate, maxSec);
      clip.name = name || blob.name || "clip";
      if (target === "speech") { A.speech.clip = clip; A.speech.clipName = A.speech.clipName || (blob.name || "My voice").replace(/\.\w+$/, ""); if (modelForTool("transcribe")?.status === "running" && !A.speech.clipText) this.autoTranscribe(); }
      if (target === "transcribe") A.transcribe.clip = clip;
      if (target === "song") A.song.ref = clip;
      this.render();
    } catch (e) { toast("Couldn't read that audio: " + e.message, "err"); }
  },
  async autoTranscribe() {
    const c = A.speech.clip; if (!c?.uri) return;
    try {
      const j = await submitJob("transcribe", { audio: c.uri, name: "voice reference", ephemeral: true }, "Transcribing reference");
      A.pendingRefText = j.id; toast("Transcribing the clip…");
    } catch (e) { toast(e.message, "err"); }
  },
  async saveVoice() {
    const sp = A.speech;
    if (!sp.clip?.uri) return;
    try {
      const v = await api("/api/voices", { method: "POST", body: JSON.stringify({ name: sp.clipName || "My voice", audio: sp.clip.uri, text: sp.clipText }) });
      sp.clip = null; sp.clipText = ""; sp.clipName = ""; sp.voiceId = v.id; store.set("audio.voice", v.id);
      toast(`Saved voice “${v.name}”`); this.loadVoices();
    } catch (e) { toast(e.message, "err"); }
  },

  /* ---------- Loop ---------- */
  loopPrompt() {
    const L = A.loop;
    if (L.custom != null) return L.custom;
    return [L.role, ...L.character, ...L.fx, ...L.phrase, `${L.bars} bars`, `${L.bpm} BPM`, L.key].join(", ");
  },
  loopPanel() {
    const L = A.loop, rerender = () => this.render();
    const toggle = (set, v) => { set.has(v) ? set.delete(v) : set.add(v); L.custom = null; rerender(); };
    const chipRow = (items, set) => el("div", { class: "chips" }, items.map((c) => el("button", { class: "chip", "aria-pressed": set.has(c), onclick: () => toggle(set, c) }, c)));
    const secs = L.bars * 4 * 60 / L.bpm;
    return [
      el("div", { class: "field" }, el("div", { class: "label" }, "Describe it", el("span", { class: "hint" }, "optional · the chat model picks the tags")),
        el("div", { class: "row" },
          el("input", { class: "input grow", placeholder: "e.g. dreamy late-night synth that slowly opens up", value: L.desc, oninput: (e) => { L.desc = e.target.value; },
            onkeydown: (e) => { if (e.key === "Enter" && !e.ctrlKey) { e.preventDefault(); this.suggestTags(); } } }),
          this.helpBtn("tags", "Suggest tags", "Suggesting…", () => this.suggestTags())),
        this.helpSlot("tags")),
      el("div", { class: "field" }, el("div", { class: "label" }, "Instrument"),
        el("div", { class: "chips" }, Object.keys(FAMILIES).map((f) => el("button", { class: "chip fam", "aria-pressed": L.family === f, onclick: () => { L.family = f; L.role = FAMILIES[f][0]; L.custom = null; rerender(); } }, f))),
        el("div", { class: "chips sub" }, FAMILIES[L.family].map((r) => el("button", { class: "chip", "aria-pressed": L.role === r, onclick: () => { L.role = r; L.custom = null; rerender(); } }, r)))),
      el("div", { class: "field" }, el("div", { class: "label" }, "Character"), chipRow(CHARACTER, L.character)),
      el("div", { class: "field" }, el("div", { class: "label" }, "Phrase"), chipRow(PHRASE, L.phrase)),
      el("div", { class: "field" }, el("div", { class: "label" }, "Effects"), chipRow(FX, L.fx)),
      el("div", { class: "field" }, el("div", { class: "row" },
        el("div", { class: "grow" }, el("div", { class: "label" }, "Tempo", el("span", { class: "hint" }, "BPM")),
          el("div", { class: "seg small" }, [100, 110, 120, 128, 130, 140, 150].map((b) => el("button", { "aria-pressed": L.bpm === b, onclick: () => { L.bpm = b; L.custom = null; rerender(); } }, String(b)))))),
        el("div", { class: "row", style: "margin-top:12px" },
          el("div", {}, el("div", { class: "label" }, "Bars"), el("div", { class: "seg small" }, [4, 8].map((b) => el("button", { "aria-pressed": L.bars === b, onclick: () => { L.bars = b; L.custom = null; rerender(); } }, String(b))))),
          el("div", { class: "grow" }, el("div", { class: "label" }, "Key"), el("select", { class: "input", onchange: (e) => { L.key = e.target.value; L.custom = null; rerender(); } }, KEYS.map((k) => el("option", { selected: k === L.key }, k)))))),
      el("div", { class: "field" }, el("div", { class: "label" }, "Prompt", el("span", { class: "hint" }, `${secs.toFixed(1)} s loop`, L.custom != null ? el("a", { href: "#", style: "margin-left:8px;color:var(--accent-2)", onclick: (e) => { e.preventDefault(); L.custom = null; rerender(); } }, "reset") : null)),
        el("div", { class: "prompt-wrap" }, el("textarea", { rows: "3", oninput: (e) => { L.custom = e.target.value; this.renderFoot(); } }, this.loopPrompt())),
        el("div", { class: "hint-line" }, "Foundation-1 works best with tag-style prompts at these exact tempos. No drums in this release.")),
      el("details", { class: "adv" }, el("summary", {}, icon("i-chev"), "Advanced", el("span", { class: "sum" }, `${L.steps} steps · cfg ${L.cfg}`)),
        el("div", { class: "adv-body" },
          this.numField("Steps", L.steps, 8, 200, (v) => { L.steps = v; }),
          this.numField("Guidance (CFG)", L.cfg, 1, 12, (v) => { L.cfg = v; }, 0.5),
          this.seedField(L))),
    ];
  },
  numField(label, val, min, max, set, step = 1) {
    const out = el("output", {}, String(val));
    return el("div", { class: "field" }, el("div", { class: "label" }, label),
      el("div", { class: "slider-row" }, el("input", { type: "range", min, max, step, value: val, oninput: (e) => { set(Number(e.target.value)); out.textContent = e.target.value; this.renderFoot(); } }), out));
  },
  seedField(obj) {
    const inp = el("input", { class: "input", inputmode: "numeric", placeholder: "random", value: obj.seed, oninput: (e) => { obj.seed = e.target.value.replace(/\D/g, ""); e.target.value = obj.seed; } });
    return el("div", { class: "field" }, el("div", { class: "label" }, "Seed", el("span", { class: "hint" }, "blank = new each run")),
      el("div", { class: "seed-row" }, inp, el("button", { class: "toggle-btn", title: "Roll", onclick: () => { obj.seed = String(Math.floor(Math.random() * 2 ** 31)); inp.value = obj.seed; } }, icon("i-dice"))));
  },

  /* ---------- Song ---------- */
  songPanel() {
    const G = A.song, rerender = () => this.render();
    const vocals = G.type !== "bgm";  // every mode except instrumental sings the lyrics
    const viaGateway = S.directAccess === false;  // e.g. the USB cable: songs go through the gateway's preview route
    const cap = el("textarea", { rows: "2", placeholder: "e.g. upbeat indie pop, female vocals, jangly guitars", oninput: (e) => { G.caption = e.target.value; this.renderFoot(); } }, G.caption);
    const lyr = el("textarea", { class: "lyrics", rows: "12", placeholder: "[verse]\nYour first verse…\n\n[chorus]\nThe hook…", oninput: (e) => { G.lyrics = e.target.value; this.renderFoot(); } }, G.lyrics);
    const insert = (tag) => { const s = lyr.selectionStart, pre = lyr.value.slice(0, s), add = (pre && !pre.endsWith("\n\n") ? (pre.endsWith("\n") ? "\n" : "\n\n") : "") + `[${tag}]\n`; lyr.setRangeText(add, s, s, "end"); lyr.focus(); G.lyrics = lyr.value; };
    const lenOut = el("output", {}, fmtSec(G.seconds));
    return [
      el("div", { class: "seg song-modes", style: "margin-top:4px" },
        [["mixed", "With vocals", "Vocals and accompaniment"], ["bgm", "Instrumental", "Accompaniment only"],
          ["vocal", "Vocals only", "A cappella vocal track"], ["separate", "Separate tracks", "The mix plus the vocal and backing tracks as separate files"]]
          .map(([t, label, title]) => el("button", { "aria-pressed": G.type === t, title, onclick: () => { G.type = t; rerender(); } }, label))),
      el("div", { class: "field" }, el("div", { class: "label" }, "Style"), el("div", { class: "prompt-wrap" }, cap),
        el("div", { class: "chips" }, SONG_CHIPS.map((c) => el("button", { class: "chip", onclick: () => { cap.value = cap.value.trim() ? cap.value.trim().replace(/,?$/, ", ") + c : c; G.caption = cap.value; this.renderFoot(); } }, "+ " + c)))),
      vocals ? el("div", { class: "field" }, el("div", { class: "label" }, "Lyrics", el("span", { class: "hint" }, "use section tags")),
        el("div", { class: "lyrics-tools" }, ["intro", "verse", "chorus", "bridge", "outro"].map((t) => el("button", { class: "chip", onclick: () => insert(t) }, `[${t}]`)),
          el("button", { class: "chip", onclick: () => { lyr.value = LYRICS_TEMPLATE; G.lyrics = lyr.value; this.renderFoot(); } }, "Example"),
          this.helpBtn("lyrics", "Write lyrics", "Writing…", () => this.writeLyrics())),
        this.helpSlot("lyrics"), lyr) : null,
      el("label", { class: "switch-row" }, el("input", { type: "checkbox", checked: !!G.preview, onchange: (e) => { G.preview = e.target.checked; rerender(); } }),
        el("span", {}, el("b", {}, "Preview first (8 s)"), el("small", {}, "Hear the opening in about a minute, then continue to the full song only if you like it"))),
      G.preview ? null : el("div", { class: "field" }, el("div", { class: "label" }, "Length"),
        el("div", { class: "slider-row" }, el("input", { type: "range", min: 15, max: 270, step: 5, value: G.seconds, oninput: (e) => { G.seconds = Number(e.target.value); lenOut.textContent = fmtSec(G.seconds); this.renderFoot(); } }), lenOut),
        viaGateway && G.type === "separate" ? el("div", { class: "hint-line warn" }, "Separate tracks need a Wi-Fi/LAN connection; over this one you'll get the mix only.") : null,
        viaGateway ? el("div", { class: "hint-line" }, "On this connection songs come from the device's gateway: shorter lengths are the opening of a full arrangement and may end mid-phrase; 4:00 or more renders a complete full-length song (takes ~15 min). Over Wi-Fi every length is a complete song.") : null),
      !SONG_STYLE_REF ? null : el("div", { class: "field" }, el("div", { class: "label" }, "Style reference", el("span", { class: "hint" }, "optional · 10–30 s of a song to borrow the feel from")),
        this.captureRow("song", G.ref), G.ref ? el("button", { class: "btn-sm ghost", style: "margin-top:6px", onclick: () => { G.ref = null; rerender(); } }, "Remove reference") : null),
      el("details", { class: "adv" }, el("summary", {}, icon("i-chev"), "Advanced", el("span", { class: "sum" }, "model defaults")),
        el("div", { class: "adv-body" }, this.seedField(G),
          el("div", { class: "field" }, el("div", { class: "label" }, "Decoder steps", el("span", { class: "hint" }, "default 10")), el("input", { class: "input", inputmode: "numeric", placeholder: "10", value: G.steps, oninput: (e) => { G.steps = e.target.value; } })),
          el("div", { class: "field" }, el("div", { class: "label" }, "Temperature", el("span", { class: "hint" }, "default 1.0")), el("input", { class: "input", inputmode: "decimal", placeholder: "1.0", value: G.temperature, oninput: (e) => { G.temperature = e.target.value; } })),
          el("div", { class: "field" }, el("div", { class: "label" }, "Top-k", el("span", { class: "hint" }, "default 250")), el("input", { class: "input", inputmode: "numeric", placeholder: "250", value: G.top_k, oninput: (e) => { G.top_k = e.target.value; } })))),
    ];
  },

  /* ---------- chat helpers ---------- */
  helpBtn(key, label, busyLabel, run) {
    const busy = A.helpBusy === key;
    return el("button", { class: "assist-btn", disabled: busy, title: "Uses the loaded chat model", onclick: run }, icon("i-spark"), busy ? busyLabel : label);
  },
  helpSlot(key) {
    const h = A.help;
    if (!h || h.key !== key) return null;
    const close = () => { A.help = null; this.render(); };
    if (h.gate) { const g = modelGate("chat", "chat"); return g ? el("div", { class: "assist-gate", "data-tool": "chat" }, g) : null; }
    return el("div", { class: "assist-note" }, `${h.what} by ${shortName(h.model)}`, el("span", { class: "spacer" }),
      el("button", { class: "linkish", onclick: () => { h.undo(); close(); } }, "Undo"), el("button", { class: "linkish", onclick: close }, "✕"));
  },
  async runHelper(key, task, payload, label, apply) {
    if (A.helpBusy) return;
    if (modelGate("chat", "chat")) { A.help = { key, gate: true }; return this.render(); }
    A.help = null; A.helpBusy = key; this.render();
    const model = modelForTool("chat").id;
    const text = await assist(task, payload, el("div"), label);
    A.helpBusy = null;
    if (text != null) A.help = { key, model, ...apply(text) };
    this.render();
  },
  writeLyrics() {
    const G = A.song;
    if (!G.caption.trim()) return toast("Describe the style first: the lyrics follow it");
    const shape = G.seconds <= 45 ? "one verse and one chorus" : G.seconds <= 120 ? "verse, chorus, verse, chorus" : "verse, chorus, verse, chorus, bridge, chorus";
    const draft = G.lyrics.trim() && G.lyrics.trim() !== LYRICS_TEMPLATE ? `\nBuild on these ideas or draft lines:\n${G.lyrics.trim()}` : "";
    this.runHelper("lyrics", "lyrics", { text: `Song style: ${G.caption.trim()}\nLength: about ${G.seconds} seconds, so ${shape}.${draft}` }, "Writing lyrics", (text) => {
      const before = G.lyrics;
      G.lyrics = text;
      return { what: "Lyrics written", undo: () => { G.lyrics = before; } };
    });
  },
  suggestTags() {
    const L = A.loop;
    if (!L.desc.trim()) return toast("Describe the loop in your own words first");
    const vocab = `Vocabulary\nInstruments: ${Object.values(FAMILIES).flat().join(", ")}\nCharacter: ${CHARACTER.join(", ")}\nPhrase: ${PHRASE.join(", ")}\nEffects: ${FX.join(", ")}`;
    this.runHelper("tags", "tags", { text: `Description: ${L.desc.trim()}`, context: vocab }, "Suggesting loop tags", (text) => {
      const snap = { family: L.family, role: L.role, character: new Set(L.character), phrase: new Set(L.phrase), fx: new Set(L.fx), custom: L.custom };
      const tags = text.split(",").map((t) => t.trim()).filter((t) => t && !/\b(bpm|bars?|major|minor|drums?)\b/i.test(t));
      const find = (list, t) => list.find((x) => x.toLowerCase() === t.toLowerCase());
      const fam = Object.keys(FAMILIES).find((f) => tags.some((t) => find(FAMILIES[f], t)));
      const extra = [];
      L.character.clear(); L.phrase.clear(); L.fx.clear();
      if (fam) { L.family = fam; L.role = tags.map((t) => find(FAMILIES[fam], t)).find(Boolean); }
      for (const t of tags) {
        if (fam && find(FAMILIES[fam], t)) continue;
        const hit = [[CHARACTER, L.character], [PHRASE, L.phrase], [FX, L.fx]].find(([list]) => find(list, t));
        if (hit) hit[1].add(find(hit[0], t)); else extra.push(t);
      }
      // Tags outside the chip vocabulary still go to the model, via the editable prompt.
      L.custom = null;
      if (extra.length || !fam) L.custom = [...(fam ? [L.role] : []), ...tags.filter((t) => !(fam && find(FAMILIES[fam], t))), `${L.bars} bars`, `${L.bpm} BPM`, L.key].join(", ");
      return { what: "Tags picked", undo: () => Object.assign(L, snap) };
    });
  },

  /* ---------- Transcribe ---------- */
  transcribePanel() {
    const T = A.transcribe;
    const drop = el("button", { class: "drop", onclick: () => { const i = el("input", { type: "file", accept: "audio/*,video/*" }); i.onchange = () => i.files[0] && this.takeClip("transcribe", i.files[0]); i.click(); } },
      icon("i-upload"), el("span", {}, el("b", {}, "Drop, paste or click to browse"), "WAV · MP3 · M4A · WebM · video files too"));
    fileZone(drop, { key: "transcribe", accept: /^(audio|video)\//, what: "audio", label: "audio to transcribe", take: (f) => this.takeClip("transcribe", f[0]) });
    return [
      el("div", { class: "field" }, el("div", { class: "label" }, "Audio", el("span", { class: "hint" }, "speech, singing, songs")),
        T.clip ? el("div", {}, makePlayer(T.clip.uri), el("div", { class: "row", style: "margin-top:8px" }, el("small", { class: "grow", style: "color:var(--faint)" }, `${T.clip.name} · ${fmtSec(T.clip.duration)}`), el("button", { class: "btn-sm ghost", onclick: () => { T.clip = null; this.render(); } }, "Clear"))) : drop,
        el("div", { style: "margin-top:10px" }, this.captureRow("transcribe", null))),
      el("div", { class: "field" }, el("div", { class: "label" }, "Language"),
        el("select", { class: "input", onchange: (e) => { T.language = e.target.value; } }, ASR_LANGS.map(([v, n]) => el("option", { value: v, selected: v === T.language }, n)))),
      el("div", { class: "hint-line" }, "Tip: open any voice or song in the library and hit “Transcribe” to send it here."),
    ];
  },

  files(files) {
    const f = files.find((x) => /^(audio|video)\//.test(x.type));
    if (!f) return false;
    const target = A.tool === "transcribe" ? "transcribe" : A.tool === "song" && SONG_STYLE_REF ? "song" : A.tool === "song" ? "transcribe" : "speech";
    if (target === "speech") { A.tool = "speech"; A.speech.clip = { pending: true }; }
    this.takeClip(target, f);
    return { speech: "voice sample", transcribe: "audio to transcribe", song: "style reference" }[target];
  },

  /* ---------- submit ---------- */
  payload(preview = false) {
    const seed = (o) => (/^\d+$/.test(o.seed) ? Number(o.seed) : preview ? undefined : Math.floor(Math.random() * 2 ** 31));
    if (A.tool === "speech") {
      const sp = A.speech, v = A.voices.find((x) => x.id === sp.voiceId);
      const useClip = sp.clip?.uri;
      if (!sp.text.trim()) return { ok: false, why: "Type something to say" };
      if (!useClip && !v) return { ok: false, why: "Pick or record a voice" };
      if (!(useClip ? sp.clipText : v.text)?.trim()) return { ok: false, why: "Add what's said in the voice clip — cloning needs its transcript" };
      return { ok: true, est: 2 + sp.text.length / 16, kind: "speech", label: "Voice: " + sp.text.slice(0, 40),
        body: { text: sp.text, ref_audio: useClip ? sp.clip.uri : "/images/" + v.file, ref_text: useClip ? sp.clipText : v.text, voice_name: useClip ? (sp.clipName || "new clip") : v.name, language: sp.language, instruct: sp.instruct } };
    }
    if (A.tool === "loop") {
      const L = A.loop, secs = L.bars * 4 * 60 / L.bpm, prompt = this.loopPrompt().trim();
      if (!prompt) return { ok: false, why: "Describe the loop" };
      return { ok: true, est: secs * 1.8 * (L.steps / 64), kind: "loop", label: "Loop: " + L.role,
        body: { prompt, seconds_total: secs, steps: L.steps, cfg_scale: L.cfg, seed: seed(L), bpm: L.bpm, bars: L.bars, key: L.key } };
    }
    if (A.tool === "song") {
      const G = A.song;
      if (!G.caption.trim()) return { ok: false, why: "Describe the style" };
      if (G.type !== "bgm" && !G.lyrics.trim()) return { ok: false, why: "Add lyrics, or switch to Instrumental" };
      if (G.ref && G.ref.duration < 10) return { ok: false, why: "The style reference must be at least 10 seconds long" };
      const body = { caption: G.caption.trim(), generation_type: G.type, seconds_total: G.seconds, seed: seed(G) };
      if (G.type !== "bgm") body.lyrics = G.lyrics.trim();
      if (G.preview) body.preview = true;
      if (G.steps) body.steps = Number(G.steps);
      if (G.temperature) body.temperature = Number(G.temperature);
      if (G.top_k) body.top_k = Number(G.top_k);
      if (SONG_STYLE_REF && G.ref) body.reference_audio = G.ref.uri;
      const est = G.preview ? 50 : S.directAccess === false ? (G.seconds >= 240 ? 900 : 10 + G.seconds * 3.2) : G.seconds * 4.3;
      return { ok: true, est, kind: "song", label: "Song: " + G.caption.slice(0, 36), body };
    }
    const T = A.transcribe;
    if (!T.clip) return { ok: false, why: "Add audio to transcribe" };
    return { ok: true, est: 2 + T.clip.duration / 10, kind: "transcribe", label: "Transcript: " + T.clip.name, body: { audio: T.clip.uri, language: T.language, name: T.clip.name } };
  },
  async submit() {
    if ($("#aGo").disabled) return;
    const p = this.payload();
    if (!p.ok) return toast(p.why);
    try {
      if (p.body.ref_audio?.startsWith("/images/")) p.body.ref_audio = await blobToUri(await (await fetch(p.body.ref_audio)).blob());
      const j = await submitJob(p.kind, { ...p.body, est: p.est }, p.label);
      A.view = { job: j.id };
      this.renderStage(); this.renderStrip();
    } catch (e) { toast(e.message, "err"); }
  },
  jobDone(j) {
    if (A.pendingRefText === j.id) {
      A.pendingRefText = null;
      if (j.status === "done" && A.speech.clip) { A.speech.clipText = j.entry.text; toast("Transcript filled in — check it, then save the voice"); }
      this.render(); return;
    }
    if (A.view?.job === j.id) A.view = j.status === "done" ? { id: j.entry.id, fresh: true } : null;
    if (this.mounted && S.ws === "audio") { this.renderStage(); this.renderStrip(); }
  },
  renderJobs() {
    if (A.view?.job) this.renderStage();
    this.renderStrip();
  },

  /* ---------- stage ---------- */
  audioHistory() { return S.history.filter((e) => AUDIO_KINDS.has(e.kind) && e.params?.name !== "voice reference"); },
  renderStage() {
    const head = $("#aHead"), stage = $("#aStage");
    head.replaceChildren(); stage.replaceChildren();
    if (A.view?.job) {
      const j = S.jobs.find((x) => x.id === A.view.job);
      if (j && (j.status === "queued" || j.status === "running")) {
        head.append(el("div", { class: "crumb" }, el("b", {}, j.status === "queued" ? "Queued" : "Working"), " · " + (j.label || j.kind)));
        stage.append(pendingCard(j));
        return;
      }
      A.view = null;
    }
    const list = this.audioHistory();
    const e = (A.view && list.find((x) => x.id === A.view.id)) || (!A.view && list[0]);
    if (!e) return this.renderEmpty(stage);
    A.view = { id: e.id };
    const kindName = { speech: "Voice", loop: "Loop", song: "Song", transcript: "Transcript" }[e.kind];
    head.append(el("div", { class: "crumb" }, el("b", {}, kindName), " · " + timeAgo(e.created)), el("div", { class: "spacer" }));
    const tb = el("div", { class: "toolbar" });
    const btn = (label, ic, fn, cls = "tb") => el("button", { class: cls, onclick: fn }, icon(ic), el("span", { class: "lbl" }, label));
    if (e.kind === "transcript") {
      tb.append(...[btn("Copy", "i-copy", () => { navigator.clipboard?.writeText(e.text); toast("Copied"); }, "tb primary"),
        btn("Speak this", "i-voice", () => { A.tool = "speech"; A.speech.text = e.text; this.render(); }),
        e.sources[0] ? btn("Save as voice", "i-mic", () => this.voiceFromEntry(e)) : null].filter(Boolean));
    } else {
      const main = e.outputs[0];
      tb.append(...[el("a", { class: "tb primary", href: "/images/" + main, download: `tiiny-${e.kind}-${e.id}.wav` }, icon("i-down"), el("span", { class: "lbl" }, "Save")),
        btn("Transcribe", "i-text", () => this.sendToTranscribe("/images/" + main, kindName)),
        e.kind !== "speech" ? btn("Vary", "i-redo", () => this.reuse(e, true)) : null,
        btn("Reuse", "i-copy", () => this.reuse(e, false)),
        e.resume ? btn("Continue to full song", "i-music", () => this.continueSong(e), "tb primary") : null,
        (e.downloads || []).find((f) => f.endsWith(".mp3")) ? el("a", { class: "tb", href: "/images/" + e.downloads.find((f) => f.endsWith(".mp3")), download: `tiiny-song-${e.id}.mp3` }, icon("i-down"), el("span", { class: "lbl" }, "MP3")) : null,
        SONG_STYLE_REF && e.kind !== "song" && e.kind !== "speech" ? btn("Use as song style", "i-music", () => this.toSongRef("/images/" + main)) : null].filter(Boolean));
    }
    tb.append(el("button", { class: "tb danger", title: "Delete", onclick: () => this.del(e) }, icon("i-trash")));
    head.append(tb);

    const card = el("div", { class: "media-card" + (A.view.fresh ? " fade-in" : "") });
    const title = e.kind === "speech" ? e.params.text : e.kind === "loop" ? e.params.prompt : e.kind === "song" ? e.params.caption : e.params?.name || "Transcript";
    card.append(el("div", { class: "media-title" }, title));
    if (e.kind === "transcript") {
      card.append(el("div", { class: "transcript" }, e.text || "(no speech detected)"));
      if (e.sources[0]) card.append(el("div", { class: "src-audio" }, el("small", {}, "Source audio"), makePlayer("/images/" + e.sources[0])));
    } else {
      card.append(makePlayer("/images/" + e.outputs[0], { big: true }));
      for (const extra of e.outputs.slice(1)) card.append(el("div", { class: "track" }, el("small", {}, e.tracks?.[extra] || extra), makePlayer("/images/" + extra)));
      if (e.kind === "song" && e.params.lyrics) card.append(el("details", { class: "desc" }, el("summary", {}, "Lyrics"), el("p", {}, e.params.lyrics)));
      if (e.kind === "speech" && e.sources[0]) card.append(el("div", { class: "src-audio" }, el("small", {}, `Voice: ${e.params.voice || "reference"}`), makePlayer("/images/" + e.sources[0])));
    }
    const p = e.params || {};
    card.append(el("div", { class: "meta-line" },
      el("span", {}, el("b", {}, shortName(p.model || ""))),
      e.duration ? el("span", {}, fmtSec(e.duration) + " audio") : null,
      p.bpm ? el("span", {}, `${p.bpm} BPM · ${p.bars} bars · ${p.key}`) : null,
      p.generation_type ? el("span", {}, { bgm: "instrumental", vocal: "vocals only", separate: "separate tracks" }[p.generation_type] || "with vocals") : null,
      p.route === "gateway-preview" ? el("span", { title: "Made over a gateway-only link (USB): the opening of a full arrangement" }, "opening of a full arrangement") : null,
      p.language ? el("span", {}, p.language) : null,
      p.instruct ? el("span", {}, `“${p.instruct}”`) : null,
      p.seed != null ? el("span", {}, `seed ${p.seed}`) : null,
      el("span", {}, `made in ${fmtSec(e.seconds)}`)));
    A.view.fresh = false;
    stage.append(card);
  },
  renderEmpty(stage) {
    stage.append(el("div", { class: "empty" },
      el("div", { class: "art" }, icon("i-wave")),
      el("h2", {}, "Voices, loops, songs and transcripts"),
      el("p", {}, "Clone a voice from a few seconds of audio, build tempo-locked loops, write a full song, or turn speech into text — all on the Tiiny."),
      el("div", { class: "tool-tiles" }, TOOLS.map((t) => el("button", { class: "tile", onclick: () => { A.tool = t.id; store.set("audio.tool", t.id); this.render(); } }, icon(t.icon), el("b", {}, t.label), el("small", {}, mediaBadges(modelForTool(t.id) || { type: TOOL_TYPES[t.id] }).slice(0, 2).join(" · ")))))));
  },
  renderStrip() {
    const items = [];
    for (const j of S.jobs.filter((x) => AUDIO_KINDS.has(x.kind === "transcribe" ? "transcript" : x.kind) && (x.status === "queued" || x.status === "running") && A.pendingRefText !== x.id))
      items.push(el("button", { class: "thumb queued" + (j.status === "running" ? " run" : ""), "aria-current": A.view?.job === j.id, title: j.label, onclick: () => { A.view = { job: j.id }; this.renderStage(); this.renderStrip(); } }, j.status === "running" ? `${j.progress || "…"}${j.progress ? "%" : ""}` : "queued"));
    const list = this.audioHistory();
    const shown = galleryFilter("audio", list), again = () => this.renderStrip();
    $("#galBar-audio").replaceChildren(galleryBar("audio", shown, again));
    for (const e of shown) items.push(decorateThumb("audio", mediaThumb(e, A.view?.id === e.id, () => { A.view = { id: e.id }; this.renderStage(); this.renderStrip(); }, () => this.del(e)), e, again));
    $("#aStrip").replaceChildren(...(items.length ? items : [el("div", { class: "strip-empty" }, "Your audio will collect here.")]));
    $("#aCount").textContent = list.length || "";
  },

  /* ---------- actions ---------- */
  async sendToTranscribe(url, name) {
    A.tool = "transcribe";
    const blob = await (await fetch(url)).blob();
    await this.takeClip("transcribe", new File([blob], `${name}.wav`, { type: blob.type }));
    toast("Loaded into Transcribe — hit Transcribe");
  },
  async toSongRef(url) {
    A.tool = "song";
    const blob = await (await fetch(url)).blob();
    await this.takeClip("song", blob, "reference");
    toast("Set as the song's style reference");
  },
  async voiceFromEntry(e) {
    A.tool = "speech";
    const blob = await (await fetch("/images/" + e.sources[0])).blob();
    A.speech.clip = { pending: true }; A.speech.clipText = e.text || ""; A.speech.clipName = e.params?.name?.replace(/\.\w+$/, "") || "Voice";
    await this.takeClip("speech", blob);
    toast("Check the name and transcript, then Save voice");
  },
  async continueSong(e) {
    const p = e.params || {};
    const body = { caption: p.caption, generation_type: p.generation_type, lyrics: p.lyrics, seed: p.seed, resume_session: e.resume.session_id, from_entry: e.id };
    try {
      const j = await submitJob("song", { ...body, est: 900 }, "Song (full): " + (p.caption || "").slice(0, 30));
      delete e.resume;
      A.view = { job: j.id }; this.renderStage(); this.renderStrip();
      toast("Continuing to the full song — this takes about 15 minutes");
    } catch (err) { toast(err.message, "err"); }
  },
  reuse(e, run) {
    const p = e.params || {};
    if (e.kind === "loop") {
      A.tool = "loop"; Object.assign(A.loop, { custom: p.prompt, steps: p.steps || 64, cfg: p.cfg_scale || 6, seed: run ? "" : String(p.seed ?? "") });
      if (p.bpm) A.loop.bpm = p.bpm; if (p.bars) A.loop.bars = p.bars; if (p.key) A.loop.key = p.key;
    } else if (e.kind === "song") {
      A.tool = "song"; Object.assign(A.song, { caption: p.caption || "", lyrics: p.lyrics || "", type: p.generation_type || "mixed", seconds: p.seconds_total || 60, seed: run ? "" : String(p.seed ?? "") });
    } else if (e.kind === "speech") {
      A.tool = "speech"; Object.assign(A.speech, { text: p.text || "", language: p.language || "", instruct: p.instruct || "" });
    }
    this.render();
    if (run) this.submit(); else toast("Settings loaded");
  },
  async del(e) {
    try { await api("/api/history/" + e.id, { method: "DELETE" }); } catch (err) { return toast(err.message, "err"); }
    const list = this.audioHistory(), i = list.indexOf(e);
    S.history.splice(S.history.indexOf(e), 1);
    if (A.view?.id === e.id) { const next = list[i + 1] || list[i - 1]; A.view = next ? { id: next.id } : null; }
    this.renderStage(); this.renderStrip();
  },
};
