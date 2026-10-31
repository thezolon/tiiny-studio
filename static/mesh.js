"use strict";
/* 3D workspace: image-to-3D (Hunyuan3D, TRELLIS.2: shape -> simplify -> PBR texture -> export) with a GLB viewer. The
   options come from the loaded model's own /v1/3d/capabilities; Hunyuan3D's are the fallback. */

// Hunyuan3D shape time: diffusion is fixed (~140 s), volume decoding grows with octree resolution³ (draft 205 s, high ~690 s).
const PRECISIONS = [["draft", "Draft", 1], ["standard", "Standard", 1.75], ["high", "High", 3.35]];
const HUNYUAN_PRECISIONS = { draft: { octree_resolution: 192 }, standard: { octree_resolution: 288 }, high: { octree_resolution: 384 } };
const QUALITIES = [["fast", "Fast", 0.75], ["balance", "Balanced", 1], ["quality", "Best", 1.5]];
const FACES = [[0, "Full"], [100000, "100k"], [40000, "40k"], [10000, "10k"]];
const M = { clip: null, precision: store.get("mesh.precision", "draft"), quality: store.get("mesh.quality", "balance"),
  texture: store.get("mesh.texture", true), faces: store.get("mesh.faces", 40000), cutout: store.get("mesh.cutout", true), asset: store.get("mesh.asset", ""),
  seed: "", view: null };
let viewerLoaded = false;
/** An image model that can put a subject on a transparent background: loaded ones first, then those that say they write
 *  a real alpha channel (Qwen-Image). FLUX.2-klein Background-Remove returns the subject on white (no alpha, 0.1.38):
 *  TRELLIS.2 builds fine from that, Hunyuan3D builds the white square too. */
const realAlpha = (m) => /alpha channel|RGBA/i.test(m.desc || "");
const cutoutModel = () => (S.allModels || []).filter((m) => caps(m).edit && caps(m).transparent)
  .sort((a, b) => (a.status === "running" ? 0 : 1) - (b.status === "running" ? 0 : 1) || (realAlpha(a) ? 0 : 1) - (realAlpha(b) ? 0 : 1))[0] || null;
/** Marks a picture whose background is already transparent (some pixels see-through), so it isn't cut out again. */
async function withTransparency(img) {
  if (!/^data:image\/(png|webp)/.test(img.uri)) return img;
  const pic = new Image(); pic.src = img.uri; await pic.decode();
  const k = Math.min(1, 256 / Math.max(img.w, img.h)), w = Math.max(1, Math.round(img.w * k)), h = Math.max(1, Math.round(img.h * k));
  const c = Object.assign(document.createElement("canvas"), { width: w, height: h }), g = c.getContext("2d");
  g.drawImage(pic, 0, 0, w, h);
  const a = g.getImageData(0, 0, w, h).data;
  let clear = 0;
  for (let i = 3; i < a.length; i += 4) if (a[i] < 16) clear++;
  return Object.assign(img, { transparent: clear > (w * h) / 20 });  // over 5% see-through
}
/** The cut-out keeps the source's shape (Hunyuan3D would build a stretched subject) at about 512 px, which is all Hunyuan3D
    uses and keeps it quick (~1.5 min; 1024 px took ~5 min): the model's closest verified size, else multiples of 64. */
function cutoutSize(m, w, h) {
  const list = modelSizeList(m), r = w / h, off = (s) => Math.abs(Math.log(s[0] / s[1] / r)) * 100 + Math.abs(Math.max(...s) - 512) / 1e4;
  if (list) return list.reduce((b, s) => (off(s) < off(b) ? s : b)).join("x");
  const k = 512 / Math.max(w, h), r64 = (v) => Math.max(256, Math.round((v * k) / 64) * 64);
  return `${r64(w)}x${r64(h)}`;
}

const MeshWS = {
  ...mediaWS({ kind: "mesh", p: "m", st: M, title: "3D models", empty: "Your 3D models will collect here.",
    own: (e) => e.kind === "mesh", jobs: (j) => j.kind === "mesh",
    queued: (j) => ({ label: `${j.progress || 0}%`, run: true, view: () => ({ job: j.id, plan: M.view?.plan }) }),
    gate: () => ["mesh", "image-to-3D"] }),
  mounted: false,
  mount(root) {
    this.shell(root, { ico: "i-cube", label: "Make 3D model", est: "mEst", foot: [el("span", { id: "mMeta" }), el("span", { id: "mSteps" })] });
    if (!viewerLoaded) {
      viewerLoaded = true;
      document.head.append(el("script", { type: "module", src: "/static/vendor/model-viewer.min.js" }));
    }
  },
  render() {
    if (!this.mounted) return;
    this.renderGate();
    $("#mPanel").replaceChildren(...this.panel().filter(Boolean));  // hidden parts are null, which append would print
    this.renderFoot(); this.renderStage(); this.renderStrip();
  },
  cuts() { return M.cutout && !!cutoutModel() && !M.clip?.transparent; },
  /** The loaded 3D model's own options (/v1/3d/capabilities): its stages, precisions and qualities. Hunyuan3D's until
   *  a model has answered. */
  model3d() {
    const m = modelForTool("mesh"), c = loadedInfo(m, () => this.render()) || {};
    const stages = c.stages || ["background", "shape", "simplify", "texture", "export"];
    const op = c.output_precisions || HUNYUAN_PRECISIONS;
    const precisions = PRECISIONS.filter(([v]) => op[v] && (!op[v].effective || op[v].effective === v));  // TRELLIS.2: draft is standard
    // what each precision means: Hunyuan3D's octree size, TRELLIS.2's pipeline resolution
    const sizes = precisions.map(([v]) => op[v].octree_resolution || op[v].pipeline_type).filter(Boolean);
    const precisionHint = op.draft?.octree_resolution ? "octree " + sizes.join(" / ") : sizes.length ? sizes.join(" / ") + " px" : "";
    const qualities = QUALITIES.filter(([v]) => (c.inference_qualities || QUALITIES.map((q) => q[0])).includes(v));
    return { family: c.model || "hunyuan3d", known: !!c.model, name: shortName(m?.id || "Hunyuan3D"), simplify: stages.includes("simplify"),
      texture: stages.includes("texture") && stages.includes("export"), precisions, precisionHint, qualities,
      assetQualities: c.asset_qualities ? QUALITIES.filter(([v]) => c.asset_qualities.includes(v)) : [] };
  },
  plan() {
    const t = this.model3d();
    return [...(this.cuts() ? ["cutout"] : []), "shape", ...(M.faces && t.simplify ? ["simplify"] : []), ...(M.texture && t.texture ? ["texture", "export"] : [])];
  },
  estimate() {
    const t = this.model3d(), plan = this.plan();
    const pm = PRECISIONS.find((p) => p[0] === M.precision)?.[2] || 1, qm = QUALITIES.find((q) => q[0] === M.quality)?.[2] || 1;
    const cm = this.cuts() && cutoutModel(), cut = cm ? 85 + (cm.status === "running" ? 0 : 2 * (cm.load_time || 30)) : 0;
    const tex = plan.includes("texture"), simp = plan.includes("simplify");
    if (t.family === "trellis2")  // measured on a mug (0.1.38): shape ~200 s (1024 px about twice that), simplify 17 s, texture ~100 s, export ~45 s
      return cut + 200 * (M.precision === "high" ? 2 : 1) * qm + (simp ? 20 : 0) + (tex ? 100 * qm + 45 : 0);
    return cut + 205 * pm * qm + (simp ? 6 : 0) + (tex ? 230 * qm + 60 : 0);
  },
  renderFoot() {
    const cm = cutoutModel(), cutBlocked = this.cuts() && cm.status !== "running" && !swapOn("mesh");  // the server would refuse
    const { ready } = this.footModel("mesh", cutBlocked ? ` · ${shortName(cm.id)} not loaded` : "");
    $("#mGo").disabled = !ready || !M.clip || cutBlocked;
    $("#mEst").textContent = M.clip ? "~" + fmtSec(this.estimate()) : "";
    $("#mSteps").textContent = this.plan().join(" → ");
  },
  panel() {
    const rerender = () => this.render(), t = this.model3d();
    if (!t.precisions.some(([v]) => v === M.precision)) M.precision = t.precisions[0]?.[0] || "draft";
    if (!t.qualities.some(([v]) => v === M.quality)) M.quality = t.qualities.find(([v]) => v === "balance")?.[0] || t.qualities[0]?.[0] || "balance";
    const recent = imageHistory().slice(0, 12);
    const src = imageSlot({ img: M.clip, clear: () => { M.clip = null; rerender(); }, cls: "mesh-src", key: "mesh-src", zone: "3D source image",
      title: "Drop, paste (Ctrl+V) or click to browse", hint: "One object, plain background works best", take: (f) => this.files(f) });
    return [
      el("div", { class: "field", style: "margin-top:0" }, el("div", { class: "label" }, "Source image"), src),
      recent.length ? el("div", { class: "field" }, el("div", { class: "label" }, "From your images"),
        el("div", { class: "pick-row" }, recent.map((e) => el("button", { class: "pick", title: e.params.prompt, onclick: () => this.fromImage("/images/" + e.outputs[0]) }, el("img", { src: "/images/" + e.outputs[0], alt: "", loading: "lazy" }))))) : null,
      this.cutoutSwitch(rerender),
      t.precisions.length ? el("div", { class: "field" }, el("div", { class: "label" }, "Geometry detail", el("span", { class: "hint" }, t.precisionHint)), this.seg(t.precisions, M.precision, (v) => { M.precision = v; }, "mesh.precision")) : null,
      t.qualities.length ? el("div", { class: "field" }, el("div", { class: "label" }, "Sampling quality"), this.seg(t.qualities, M.quality, (v) => { M.quality = v; }, "mesh.quality")) : null,
      t.known && t.assetQualities.length ? el("div", { class: "field" }, el("div", { class: "label" }, "Mesh quality", el("span", { class: "hint" }, "simplify and export")),
        this.seg([["", "Default"], ...t.assetQualities], M.asset, (v) => { M.asset = v; }, "mesh.asset")) : null,
      t.simplify ? el("div", { class: "field" }, el("div", { class: "label" }, "Polygon budget", el("span", { class: "hint" }, "simplify before texturing")), this.seg(FACES, M.faces, (v) => { M.faces = v; }, "mesh.faces")) : null,
      t.texture ? el("label", { class: "switch-row" }, el("input", { type: "checkbox", checked: M.texture, onchange: (e) => { M.texture = e.target.checked; store.set("mesh.texture", M.texture); rerender(); } }),
        el("span", {}, el("b", {}, "PBR texture"), el("small", {}, `Paint color, metal and roughness maps · adds ~${t.family === "trellis2" ? "2.5" : "5"} min`))) : null,
      t.texture && M.texture && !M.faces ? el("div", { class: "hint-line warn" }, "Texturing a full-resolution mesh is slow — a 40k budget is recommended.") : null,
      el("details", { class: "adv" }, el("summary", {}, icon("i-chev"), "Advanced", el("span", { class: "sum" }, M.seed ? "seed " + M.seed : "auto")),
        el("div", { class: "adv-body" }, seedField(M))),
    ];
  },
  cutoutSwitch(rerender) {
    const cm = cutoutModel();
    const why = !cm ? "Needs an image model that makes transparent pictures (Qwen-Image)."
      : M.clip?.transparent ? "This picture already has a transparent background, so it's used as it is."
      : `${shortName(cm.id)} puts the subject on a transparent background first, so the background doesn't end up in the model · adds ~1.5 min`
        + (cm.status === "running" ? "" : swapOn("mesh") ? ", plus loading it (models are swapped back after)" : `. Load ${shortName(cm.id)} first, or turn on 3D in Settings → Model swapping.`);
    return el("label", { class: "switch-row" }, el("input", { type: "checkbox", checked: M.cutout && !!cm, disabled: !cm, onchange: (e) => { M.cutout = e.target.checked; store.set("mesh.cutout", M.cutout); rerender(); } }),
      el("span", {}, el("b", {}, "Cut out the subject first"), el("small", {}, why)));
  },
  files(files) {
    const f = files.find((x) => x.type.startsWith("image/"));
    if (!f) return false;
    readImage(f).then(withTransparency).then((img) => { M.clip = img; this.render(); }).catch((e) => toast(e.message, "err"));
    return "3D source image";
  },
  async fromImage(url) {
    const blob = await (await fetch(url)).blob();
    M.clip = await withTransparency(await readImage(new File([blob], url.split("/").pop(), { type: blob.type })));
    if (S.ws !== "mesh") setWs("mesh"); else this.render();
    toast("Loaded — pick your options and hit Make 3D model");
  },
  async submit() {
    if ($("#mGo").disabled) return;
    const t = this.model3d();
    const body = { image: M.clip.uri, output_precision: M.precision, inference_quality: M.quality, texture: M.texture && t.texture, est: this.estimate() };
    if (M.faces && t.simplify) body.faces = M.faces;
    if (M.asset && t.known && t.assetQualities.length) body.asset_quality = M.asset;
    if (this.cuts()) { const cm = cutoutModel(); Object.assign(body, { cutout: true, cutout_model: cm.id, cutout_size: cutoutSize(cm, M.clip.w, M.clip.h) }); }
    if (/^\d+$/.test(M.seed)) body.seed = Number(M.seed);
    try {
      const j = await submitJob("mesh", body, `3D: ${M.clip.name || "image"}`);
      M.view = { job: j.id, plan: this.plan(), src: M.clip.uri };
      this.renderStage(); this.renderStrip();
    } catch (e) { toast(e.message, "err"); }
  },

  renderStage() {
    const head = $("#mHead"), stage = $("#mStage");
    const j = this.liveJob();
    if (j) return this.renderPending(head, stage, j);
    head.replaceChildren(); stage.replaceChildren();
    const e = this.current(this.entries());
    if (!e) {
      stage.append(el("div", { class: "empty" }, el("div", { class: "art" }, icon("i-cube")), el("h2", {}, "Turn a picture into a 3D model"),
        el("p", {}, `${this.model3d().name} builds the shape from one image, then paints physically based textures you can drop into Blender, Unity or Unreal.`),
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
        el("button", { class: "tb danger", title: "Delete", "aria-label": "Delete", onclick: () => this.del(e) }, icon("i-trash"))));
    const viewer = document.createElement("model-viewer");
    Object.entries({ src: url, "camera-controls": "", ...(REDUCED_MOTION ? {} : { "auto-rotate": "" }), "shadow-intensity": "1", exposure: "1.05", "environment-image": "neutral", "interaction-prompt": "none", alt: "3D model" })
      .forEach(([k, v]) => viewer.setAttribute(k, v));
    viewer.className = "viewer";
    const stats = (e.stages || []).map((s) => s.metadata).filter(Boolean);
    const faces = stats.find((s) => s.output_faces)?.output_faces;
    stage.append(el("div", { class: "mesh-result" },
      el("div", { class: "viewer-wrap" }, viewer, e.sources[0] ? el("img", { class: "viewer-src", src: "/images/" + (e.sources[1] || e.sources[0]), alt: "source", title: e.sources[1] ? "The cut-out the model was built from" : "Source image" }) : null),
      el("div", { class: "meta-line" },
        el("span", {}, el("b", {}, shortName(p.model || ""))),
        el("span", {}, p.texture ? "PBR textured" : "shape only"),
        p.cutout ? el("span", {}, `cut out by ${shortName(p.cutout)}`) : null,
        p.output_precision ? el("span", {}, p.output_precision + " detail") : null,
        faces ? el("span", {}, `${(faces / 1000).toFixed(0)}k faces`) : null,
        el("span", {}, `made in ${fmtSec(e.seconds)}`),
        el("span", {}, "drag to orbit · scroll to zoom"))));
  },
  renderPending(head, stage, j) {
    const plan = M.view.plan || ["shape", "simplify", "texture", "export"];
    const t = j.started ? Date.now() / 1000 - j.started : 0;
    const names = { cutout: "Cut out", background: "Background", shape: "Shape", simplify: "Simplify", texture: "Texture", export: "Export" };
    const cur = plan.indexOf(j.stage || plan[0]);
    head.replaceChildren(el("div", { class: "crumb" }, el("b", {}, j.status === "queued" ? "Queued" : "Building 3D model"), " · " + (j.label || "")));
    const box = stage.querySelector(".mesh-pending") || (() => { stage.replaceChildren(); const b = el("div", { class: "pending mesh-pending" }); stage.append(b); return b; })();
    box.replaceChildren(...[
      M.view.src ? el("img", { class: "src-ghost", src: M.view.src, alt: "" }) : null,
      el("div", { class: "pending-inner" },
        el("div", { class: "big" }, j.status === "queued" ? "Waiting" : fmtSec(t)),
        el("div", { class: "sub" }, j.status === "queued" ? `#${j.queue_pos || 1} in the device queue` : j.message || j.status),
        el("div", { class: "bar" }, el("i", { style: `width:${j.progress || 0}%` })),
        el("ol", { class: "pipeline" }, plan.map((s, i) => el("li", { class: i < cur ? "done" : i === cur && j.status === "running" ? "now" : "" }, names[s]))),
        el("div", { class: "prompt-echo" }, j.est ? `usually about ${fmtSec(j.est)}` : ""),
        el("button", { class: "tb cancel", disabled: !!j.stopping, title: j.stopping ? STOPPING : "", ...onPress((b) => stopButton(b, j.id)) }, j.stopping ? "Stopping…" : "Cancel"))].filter(Boolean));
  },
  async reuse(e) {
    const p = e.params;
    Object.assign(M, { precision: p.output_precision || M.precision, quality: p.inference_quality || M.quality, texture: !!p.texture, faces: p.faces || 0 });
    if (e.sources[0]) await this.fromImage("/images/" + e.sources[0]); else this.render();
  },
};
