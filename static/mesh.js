"use strict";
/* 3D workspace: Hunyuan3D image-to-3D (shape -> simplify -> PBR texture -> export) with a GLB viewer. */

// Shape time: diffusion is fixed (~140 s), volume decoding grows with octree resolution³ (measured: draft 205 s, high ~690 s).
const PRECISIONS = [["draft", "Draft", 1], ["standard", "Standard", 1.75], ["high", "High", 3.35]];
const QUALITIES = [["fast", "Fast", 0.75], ["balance", "Balanced", 1], ["quality", "Best", 1.5]];
const FACES = [[0, "Full"], [100000, "100k"], [40000, "40k"], [10000, "10k"]];
const M = { clip: null, precision: store.get("mesh.precision", "draft"), quality: store.get("mesh.quality", "balance"),
  texture: store.get("mesh.texture", true), faces: store.get("mesh.faces", 40000), seed: "", view: null };
let viewerLoaded = false;

const MeshWS = {
  mounted: false,
  mount(root) {
    root.replaceChildren(
      el("aside", { class: "composer" },
        el("div", { class: "composer-scroll" }, el("div", { id: "mGate" }), el("div", { id: "mPanel" })),
        el("div", { class: "composer-foot" },
          el("button", { class: "go", id: "mGo", onclick: () => this.submit() }, icon("i-cube"), el("span", {}, "Make 3D model"), el("span", { class: "est", id: "mEst" })),
          el("div", { class: "foot-meta" }, el("span", { id: "mMeta" }), el("span", { id: "mSteps" })))),
      el("section", { class: "stage-col" },
        el("div", { class: "stage-head", id: "mHead" }),
        el("div", { class: "stage", id: "mStage" }),
        el("div", { class: "strip-wrap" },
          el("div", { class: "strip-head" }, "3D models ", el("span", { class: "count", id: "mCount" }), el("span", { id: "galBar-mesh" })),
          el("div", { class: "strip", id: "mStrip" }))));
    if (!viewerLoaded) {
      viewerLoaded = true;
      document.head.append(el("script", { type: "module", src: "/static/vendor/model-viewer.min.js" }));
    }
  },
  render() {
    if (!this.mounted) return;
    this.renderGate();
    $("#mPanel").replaceChildren(...this.panel());
    this.renderFoot(); this.renderStage(); this.renderStrip();
  },
  renderGate() {
    const g = modelGate("mesh", "image-to-3D");
    $("#mGate").replaceChildren(...(g ? [g] : []));
    this.renderFoot();
  },
  plan() { return ["shape", ...(M.faces ? ["simplify"] : []), ...(M.texture ? ["texture", "export"] : [])]; },
  estimate() {
    const pm = PRECISIONS.find((p) => p[0] === M.precision)[2], qm = QUALITIES.find((q) => q[0] === M.quality)[2];
    return 205 * pm * qm + (M.faces ? 6 : 0) + (M.texture ? 230 * qm + 60 : 0);
  },
  renderFoot() {
    const m = modelForTool("mesh"), ready = m?.status === "running";
    $("#mGo").disabled = !ready || !M.clip;
    $("#mEst").textContent = M.clip ? "~" + fmtSec(this.estimate()) : "";
    $("#mMeta").textContent = m ? shortName(m.id) + (ready ? "" : " · not loaded") : "";
    $("#mSteps").textContent = this.plan().join(" → ");
  },
  panel() {
    const rerender = () => this.render();
    const recent = imageHistory().slice(0, 12);
    const src = M.clip
      ? el("div", { class: "mesh-src" }, el("img", { src: M.clip.uri, alt: "" }),
        el("button", { class: "x", title: "Remove", onclick: () => { M.clip = null; rerender(); } }, "✕"))
      : (() => { const d = el("button", { class: "drop", onclick: () => { const i = el("input", { type: "file", accept: "image/*" }); i.onchange = () => i.files[0] && this.files([i.files[0]]); i.click(); } },
        icon("i-upload"), el("span", {}, el("b", {}, "Drop, paste (Ctrl+V) or click to browse"), "One object, plain background works best"));
        return fileZone(d, { key: "mesh-src", accept: /^image\//, what: "image", label: "3D source image", take: (f) => this.files(f) }); })();
    const seg = (items, val, set, key) => el("div", { class: "seg small" }, items.map(([v, label]) => el("button", { "aria-pressed": val === v, onclick: () => { set(v); store.set(key, v); rerender(); } }, label)));
    return [
      el("div", { class: "field", style: "margin-top:0" }, el("div", { class: "label" }, "Source image"), src),
      recent.length ? el("div", { class: "field" }, el("div", { class: "label" }, "From your images"),
        el("div", { class: "pick-row" }, recent.map((e) => el("button", { class: "pick", title: e.params.prompt, onclick: () => this.fromImage("/images/" + e.outputs[0]) }, el("img", { src: "/images/" + e.outputs[0], alt: "", loading: "lazy" }))))) : null,
      el("div", { class: "hint-line" }, "Tip: in Image, turn on “Transparent background” (Qwen-Image) to cut the object out first.",
        el("a", { href: "#", style: "margin-left:6px;color:var(--accent-2)", onclick: (e) => { e.preventDefault(); setWs("image"); } }, "Open Image")),
      el("div", { class: "field" }, el("div", { class: "label" }, "Geometry detail", el("span", { class: "hint" }, "octree 192 / 288 / 384")), seg(PRECISIONS, M.precision, (v) => { M.precision = v; }, "mesh.precision")),
      el("div", { class: "field" }, el("div", { class: "label" }, "Sampling quality"), seg(QUALITIES, M.quality, (v) => { M.quality = v; }, "mesh.quality")),
      el("div", { class: "field" }, el("div", { class: "label" }, "Polygon budget", el("span", { class: "hint" }, "simplify before texturing")), seg(FACES, M.faces, (v) => { M.faces = v; }, "mesh.faces")),
      el("label", { class: "switch-row" }, el("input", { type: "checkbox", checked: M.texture, onchange: (e) => { M.texture = e.target.checked; store.set("mesh.texture", M.texture); rerender(); } }),
        el("span", {}, el("b", {}, "PBR texture"), el("small", {}, "Paint color, metal and roughness maps · adds ~5 min"))),
      M.texture && !M.faces ? el("div", { class: "hint-line warn" }, "Texturing a full-resolution mesh is slow — a 40k budget is recommended.") : null,
      el("details", { class: "adv" }, el("summary", {}, icon("i-chev"), "Advanced", el("span", { class: "sum" }, M.seed ? "seed " + M.seed : "auto")),
        el("div", { class: "adv-body" }, AudioWS.seedField(M))),
    ];
  },
  files(files) {
    const f = files.find((x) => x.type.startsWith("image/"));
    if (!f) return false;
    readImage(f).then((img) => { M.clip = img; this.render(); }).catch((e) => toast(e.message, "err"));
    return "3D source image";
  },
  async fromImage(url) {
    const blob = await (await fetch(url)).blob();
    M.clip = await readImage(new File([blob], url.split("/").pop(), { type: blob.type }));
    if (S.ws !== "mesh") setWs("mesh"); else this.render();
    toast("Loaded — pick your options and hit Make 3D model");
  },
  async submit() {
    if ($("#mGo").disabled) return;
    const body = { image: M.clip.uri, output_precision: M.precision, inference_quality: M.quality, texture: M.texture, est: this.estimate() };
    if (M.faces) body.faces = M.faces;
    if (/^\d+$/.test(M.seed)) body.seed = Number(M.seed);
    try {
      const j = await submitJob("mesh", body, `3D: ${M.clip.name || "image"}`);
      M.view = { job: j.id, plan: this.plan(), src: M.clip.uri };
      this.renderStage(); this.renderStrip();
    } catch (e) { toast(e.message, "err"); }
  },
  jobDone(j) {
    if (M.view?.job === j.id) M.view = j.status === "done" ? { id: j.entry.id } : null;
    if (this.mounted && S.ws === "mesh") { this.renderStage(); this.renderStrip(); }
  },
  renderJobs() { if (M.view?.job) this.renderStage(); this.renderStrip(); },
  meshHistory() { return S.history.filter((e) => e.kind === "mesh"); },

  renderStage() {
    const head = $("#mHead"), stage = $("#mStage");
    if (M.view?.job) {
      const j = S.jobs.find((x) => x.id === M.view.job);
      if (j && (j.status === "queued" || j.status === "running")) return this.renderPending(head, stage, j);
      M.view = null;
    }
    head.replaceChildren(); stage.replaceChildren();
    const list = this.meshHistory();
    const e = (M.view && list.find((x) => x.id === M.view.id)) || (!M.view && list[0]);
    if (!e) {
      stage.append(el("div", { class: "empty" }, el("div", { class: "art" }, icon("i-cube")), el("h2", {}, "Turn a picture into a 3D model"),
        el("p", {}, "Hunyuan3D builds the shape from one image, then paints physically based textures you can drop into Blender, Unity or Unreal."),
        el("div", { class: "steps" }, el("span", {}, el("b", {}, "1"), "Pick an image"), el("span", {}, el("b", {}, "2"), "Shape"), el("span", {}, el("b", {}, "3"), "Texture"), el("span", {}, el("b", {}, "4"), "Download GLB"))));
      return;
    }
    M.view = { id: e.id };
    const url = "/images/" + e.outputs[0], p = e.params;
    head.append(el("div", { class: "crumb" }, el("b", {}, "3D model"), " · " + timeAgo(e.created)), el("div", { class: "spacer" }),
      el("div", { class: "toolbar" },
        el("a", { class: "tb primary", href: url, download: `tiiny-3d-${e.id}.glb` }, icon("i-down"), el("span", { class: "lbl" }, "Download GLB")),
        e.sources[0] ? el("button", { class: "tb", onclick: () => { setWs("image"); useAsSource("/images/" + e.sources[0]); } }, icon("i-brush"), el("span", { class: "lbl" }, "Edit source")) : null,
        el("button", { class: "tb", onclick: () => this.reuse(e) }, icon("i-copy"), el("span", { class: "lbl" }, "Reuse")),
        el("button", { class: "tb danger", title: "Delete", onclick: () => this.del(e) }, icon("i-trash"))));
    const viewer = document.createElement("model-viewer");
    Object.entries({ src: url, "camera-controls": "", "auto-rotate": "", "shadow-intensity": "1", exposure: "1.05", "environment-image": "neutral", "interaction-prompt": "none", alt: "3D model" })
      .forEach(([k, v]) => viewer.setAttribute(k, v));
    viewer.className = "viewer";
    const stats = (e.stages || []).map((s) => s.metadata).filter(Boolean);
    const faces = stats.find((s) => s.output_faces)?.output_faces;
    stage.append(el("div", { class: "mesh-result" },
      el("div", { class: "viewer-wrap" }, viewer, e.sources[0] ? el("img", { class: "viewer-src", src: "/images/" + e.sources[0], alt: "source", title: "Source image" }) : null),
      el("div", { class: "meta-line" },
        el("span", {}, el("b", {}, shortName(p.model || ""))),
        el("span", {}, p.texture ? "PBR textured" : "shape only"),
        p.output_precision ? el("span", {}, p.output_precision + " detail") : null,
        faces ? el("span", {}, `${(faces / 1000).toFixed(0)}k faces`) : null,
        el("span", {}, `made in ${fmtSec(e.seconds)}`),
        el("span", {}, "drag to orbit · scroll to zoom"))));
  },
  renderPending(head, stage, j) {
    const plan = M.view.plan || ["shape", "simplify", "texture", "export"];
    const t = j.started ? Date.now() / 1000 - j.started : 0;
    const names = { background: "Background", shape: "Shape", simplify: "Simplify", texture: "Texture", export: "Export" };
    const cur = plan.indexOf(j.stage || plan[0]);
    head.replaceChildren(el("div", { class: "crumb" }, el("b", {}, j.status === "queued" ? "Queued" : "Building 3D model"), " · " + (j.label || "")));
    const box = stage.querySelector(".mesh-pending") || (() => { stage.replaceChildren(); const b = el("div", { class: "pending mesh-pending" }); stage.append(b); return b; })();
    box.replaceChildren(
      M.view.src ? el("img", { class: "src-ghost", src: M.view.src, alt: "" }) : null,
      el("div", { class: "pending-inner" },
        el("div", { class: "big" }, j.status === "queued" ? "Waiting" : fmtSec(t)),
        el("div", { class: "sub" }, j.status === "queued" ? `#${j.queue_pos || 1} in the device queue` : j.message || j.status),
        el("div", { class: "bar" }, el("i", { style: `width:${j.progress || 0}%` })),
        el("ol", { class: "pipeline" }, plan.map((s, i) => el("li", { class: i < cur ? "done" : i === cur && j.status === "running" ? "now" : "" }, names[s]))),
        el("div", { class: "prompt-echo" }, j.est ? `usually about ${fmtSec(j.est)}` : ""),
        el("button", { class: "tb cancel", onclick: () => cancelJob(j.id) }, "Cancel")));
  },
  renderStrip() {
    const items = [];
    for (const j of S.jobs.filter((x) => x.kind === "mesh" && (x.status === "queued" || x.status === "running")))
      items.push(el("button", { class: "thumb queued run", "aria-current": M.view?.job === j.id, onclick: () => { M.view = { job: j.id, plan: M.view?.plan }; this.renderStage(); this.renderStrip(); } }, `${j.progress || 0}%`));
    const list = this.meshHistory();
    const shown = galleryFilter("mesh", list), again = () => this.renderStrip();
    $("#galBar-mesh").replaceChildren(galleryBar("mesh", shown, again));
    for (const e of shown) items.push(decorateThumb("mesh", mediaThumb(e, M.view?.id === e.id, () => { M.view = { id: e.id }; this.renderStage(); this.renderStrip(); }, () => this.del(e)), e, again));
    $("#mStrip").replaceChildren(...(items.length ? items : [el("div", { class: "strip-empty" }, "Your 3D models will collect here.")]));
    $("#mCount").textContent = list.length || "";
  },
  async reuse(e) {
    const p = e.params;
    Object.assign(M, { precision: p.output_precision || M.precision, quality: p.inference_quality || M.quality, texture: !!p.texture, faces: p.faces || 0 });
    if (e.sources[0]) await this.fromImage("/images/" + e.sources[0]); else this.render();
  },
  async del(e) {
    try { await api("/api/history/" + e.id, { method: "DELETE" }); } catch (err) { return toast(err.message, "err"); }
    S.history.splice(S.history.indexOf(e), 1);
    if (M.view?.id === e.id) M.view = null;
    this.renderStage(); this.renderStrip();
  },
};
