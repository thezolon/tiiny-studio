"use strict";
/* Video workspace: text or a start image (and optionally an end frame) to a short clip with sound (MiniMax-H3-Turbo).
   The sizes, steps and frame rate come from the loaded model's own /v1/video/info; MiniMax-H3's are the fallback. */

TOOL_TYPES.video = "Text-to-Video";
const VIDEO_INFO = { canvases: [{ width: 832, height: 480 }, { width: 480, height: 832 }, { width: 1344, height: 768 }, { width: 768, height: 1344 }],
  step_options: [4, 5, 6, 7, 8], default_steps: 7, fps: 24, max_keyframes: 2 };
// MiniMax-H3-Turbo is compiled for clips up to 7 s; 8 s and longer fail at once ("no exported bucket fits", 0.1.38)
const LENGTHS = [[2, "2 s"], [3, "3 s"], [5, "5 s"], [7, "7 s"]];
const V = { prompt: store.get("video.prompt", ""), size: store.get("video.size", "832x480"), steps: store.get("video.steps", 4),
  seconds: store.get("video.seconds", 2), first: null, seed: "", view: null };

const VideoWS = {
  ...mediaWS({ kind: "video", p: "v", st: V, title: "Videos", empty: "Your videos will collect here.",
    own: (e) => e.kind === "video", jobs: (j) => j.kind === "video",
    queued: (j) => ({ label: `${j.progress || 0}%`, run: true }),
    gate: () => ["video", "video"],
    thumb: (t, e) => { t.title = `${e.params?.prompt || (e.params?.recovered ? "Recovered from the Tiiny" : "Video")}
${e.params?.recovered ? "" : `Took ${fmtSec(e.seconds)} to render · `}${e.params?.size || ""}, ${e.params?.steps || "default"} steps`; } }),
  mounted: false,
  mount(root) {
    this.shell(root, { ico: "i-film", label: "Make video", est: "vEst", foot: [el("span", { id: "vMeta" }), el("span", {}, el("kbd", {}, "Ctrl"), " ", el("kbd", {}, "Enter"))] });
  },
  render() {
    if (!this.mounted) return;
    this.renderGate();
    $("#vPanel").replaceChildren(...this.panel().filter(Boolean));
    this.renderFoot(); this.renderStage(); this.renderStrip();
  },
  /** The loaded video model's own options (/v1/video/info), MiniMax-H3's until it has answered. */
  info() {
    const m = modelForTool("video"), i = loadedInfo(m, () => this.render()) || {};
    const canvases = (i.canvases || VIDEO_INFO.canvases).map((c) => `${c.width}x${c.height}`);
    const steps = i.step_options || VIDEO_INFO.step_options;
    return { name: shortName(m?.id || "MiniMax-H3"), canvases, steps, defSteps: i.default_steps || VIDEO_INFO.default_steps,
      fps: i.fps || VIDEO_INFO.fps, keyframes: i.max_keyframes ?? VIDEO_INFO.max_keyframes, audio: !!i.audio || !i.fps };
  },
  /** Measured on 0.1.38 (4 steps): 2 s at 832x480 435 s, 7 s at 832x480 737 s, 5 s at 1344x768 1916 s; 2 s at 832x480 with
   *  7 steps just over 15 min. A clip costs ~5 min whatever its length, then ~1 min per second at 480p; pixels cost more
   *  than linearly (768p is ~5x 480p per second). */
  estimate() {
    const t = this.info(), [w, h] = V.size.split("x").map(Number), steps = V.steps || t.defSteps;
    return (314 + 60.4 * V.seconds * ((w * h) / (832 * 480)) ** 1.75) * (steps / 4) ** 1.4;
  },
  renderFoot() {
    const { ready } = this.footModel("video");
    $("#vGo").disabled = !ready || !(V.prompt.trim() || V.first);
    $("#vEst").textContent = ready ? "~" + fmtSec(this.estimate()) : "";
  },
  panel() {
    const rerender = () => this.render(), t = this.info();
    if (!t.canvases.includes(V.size)) V.size = t.canvases.find((c) => c === "832x480") || t.canvases[0];
    if (V.steps && !t.steps.includes(V.steps)) V.steps = 0;
    const sizeLabel = (c) => { const [w, h] = c.split("x").map(Number); return `${w > h ? "Wide" : "Tall"} ${Math.min(w, h)}p`; };
    const frame = (which, label) => imageSlot({ img: V[which], clear: () => { V[which] = null; rerender(); }, cls: "mesh-src video-frame", key: "video-" + which,
      zone: label, title: label, hint: "Drop, paste or click", take: (f) => this.take(which, f[0]) });
    const prompt = el("textarea", { rows: "4", placeholder: "What happens in the clip: subject, motion, camera, sound…",
      oninput: (e) => { V.prompt = e.target.value; store.set("video.prompt", V.prompt); this.renderFoot(); },
      onkeydown: (e) => { if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) this.submit(); } }, V.prompt);
    return [
      el("div", { class: "hint-line warn", style: "margin-top:0" }, `Video is slow on the Tiiny: about ${fmtSec(this.estimate())} for these settings `
        + "(2 s at 480p with 4 steps took 7 minutes). The device does nothing else meanwhile; start small."),
      el("div", { class: "field", style: "margin-top:0" }, el("div", { class: "label" }, "Prompt"), el("div", { class: "prompt-wrap" }, prompt)),
      t.keyframes ? el("div", { class: "field" }, el("div", { class: "label" }, "Start from a picture", el("span", { class: "hint" }, "optional")),
        el("div", { class: "video-frames" }, frame("first", "First frame"))) : null,
      el("div", { class: "field" }, el("div", { class: "label" }, "Size"), this.seg(t.canvases.map((c) => [c, sizeLabel(c)]), V.size, (v) => { V.size = v; }, "video.size")),
      el("div", { class: "field" }, el("div", { class: "label" }, "Length", el("span", { class: "hint" }, `${t.fps} fps${t.audio ? " · with sound" : ""}`)),
        this.seg(LENGTHS, V.seconds, (v) => { V.seconds = v; }, "video.seconds")),
      el("details", { class: "adv" }, el("summary", {}, icon("i-chev"), "Advanced", el("span", { class: "sum" }, `${V.steps || t.defSteps} steps` + (V.seed ? " · seed " + V.seed : ""))),
        el("div", { class: "adv-body" },
          el("div", { class: "field" }, el("div", { class: "label" }, "Steps", el("span", { class: "hint" }, "fewer is faster")),
            this.seg([[0, `Default (${t.defSteps})`], ...t.steps.map((s) => [s, String(s)])], V.steps, (v) => { V.steps = v; }, "video.steps")),
          seedField(V))),
    ];
  },
  take(which, f) {
    if (!f?.type?.startsWith("image/")) return false;
    readImage(f).then((img) => { V[which] = img; if (S.ws !== "video") setWs("video"); else this.render(); }).catch((e) => toast(e.message, "err"));
    return "video frame";
  },
  files(files) { return this.take("first", files.find((x) => x.type.startsWith("image/"))); },
  async fromImage(url) {
    const blob = await (await fetch(url)).blob();
    this.take("first", new File([blob], url.split("/").pop(), { type: blob.type }));
    toast("Loaded as the first frame — describe the motion and hit Make video");
  },
  async submit() {
    if ($("#vGo").disabled) return;
    const body = { prompt: V.prompt.trim(), size: V.size, seconds: V.seconds, est: this.estimate() };
    if (V.steps) body.steps = V.steps;
    if (V.first) body.image = V.first.uri;
    if (/^\d+$/.test(V.seed)) body.seed = Number(V.seed);
    try {
      const j = await submitJob("video", body, `Video: ${(body.prompt || V.first?.name || "clip").slice(0, 60)}`);
      V.view = { job: j.id, src: V.first?.uri };
      this.renderStage(); this.renderStrip();
    } catch (e) { toast(e.message, "err"); }
  },

  renderStage() {
    const head = $("#vHead"), stage = $("#vStage");
    const j = this.liveJob();
    if (j) {
      head.replaceChildren(el("div", { class: "crumb" }, el("b", {}, j.status === "queued" ? "Queued" : "Making video"), " · " + (j.label || "")));
      return stage.replaceChildren(pendingCard(j));
    }
    head.replaceChildren(); stage.replaceChildren();
    const e = this.current(this.entries());
    if (!e) {
      stage.append(el("div", { class: "empty" }, el("div", { class: "art" }, icon("i-film")), el("h2", {}, "Make a short video"),
        el("p", {}, `${this.info().name} turns a prompt, or a picture to start from, into a clip with matching sound. `
          + "It's slow on the Tiiny: minutes per second of video, so try 2 s at 480p first."),
        el("div", { class: "steps" }, el("span", {}, el("b", {}, "1"), "Describe it"), el("span", {}, el("b", {}, "2"), "Add a first frame (optional)"), el("span", {}, el("b", {}, "3"), "Download MP4"))));
      return;
    }
    V.view = { id: e.id };
    const url = "/images/" + e.outputs[0], p = e.params;
    head.append(el("div", { class: "crumb" }, el("b", {}, "Video"), " · " + timeAgo(e.created)), el("div", { class: "spacer" }),
      el("div", { class: "toolbar" },
        el("a", { class: "tb primary", href: url, download: `tiiny-video-${e.id}.mp4` }, icon("i-down"), el("span", { class: "lbl" }, "Download MP4")),
        el("button", { class: "tb", onclick: () => this.reuse(e) }, icon("i-copy"), el("span", { class: "lbl" }, "Reuse")),
        el("button", { class: "tb danger", title: "Delete", onclick: () => this.del(e) }, icon("i-trash"))));
    stage.append(el("div", { class: "video-result" },
      el("video", { class: "video-player", src: url, controls: true, autoplay: true, loop: true, playsinline: true }),
      p.prompt || p.recovered ? el("div", { class: "prompt-echo" }, p.prompt || "Recovered from the Tiiny") : null,
      el("div", { class: "meta-line" },
        el("span", {}, el("b", {}, p.recovered ? "Saved from the Tiiny after it finished" : `This clip took ${fmtSec(e.seconds)} to render`)),
        el("span", {}, shortName(p.model || "")),
        el("span", {}, p.size), el("span", {}, `${e.duration || p.seconds} s`), el("span", {}, p.steps ? `${p.steps} steps` : "default steps"),
        e.sources?.length ? el("span", {}, "from a picture") : null,
        p.seed != null ? el("span", {}, "seed " + p.seed) : null)));
  },
  async reuse(e) {
    const p = e.params;
    Object.assign(V, { prompt: p.prompt || "", size: p.size || V.size, seconds: p.seconds || V.seconds, steps: p.steps || 0, first: null });
    if (e.sources?.[0]) {
      const blob = await (await fetch("/images/" + e.sources[0])).blob();
      V.first = await readImage(new File([blob], e.sources[0], { type: blob.type }));
    }
    this.render();
  },
};
