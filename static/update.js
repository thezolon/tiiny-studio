"use strict";
/* "A new version is out": a quiet pill in the top bar, a dialog with the release notes, and the Settings switch.
   The server does the asking (GitHub, at most once a day, anonymous; off with Settings → Check for updates); this
   page only asks the server. Nothing is downloaded or installed: the dialog links to the release page. */

const UPD = { info: null };
const isNumericVersion = (v) => /^\d+(\.\d+)*$/.test(v);
const safeLink = (u) => (/^https?:\/\//i.test(u || "") ? u : "");

/** First launch (not Docker): ask once before GitHub is ever contacted, in a small notice that doesn't block the page.
    Yes and No are saved; closing it with the cross saves nothing, so the question comes again at the next start. */
async function askUpdateCheck() {
  let info;
  try { info = await api("/api/settings"); } catch { return; }
  if (!info?.update_ask) return;
  const answer = async (on) => {
    try { await api("/api/settings/updates", { method: "POST", body: JSON.stringify({ update_check: on }) }); }
    catch (e) { toast(e.message, "err"); return; }
    note.remove();
    if (on) checkUpdate();
  };
  const note = el("div", { class: "toast notice", role: "group", "aria-label": "Check for updates automatically?" },
    el("b", {}, "Check for updates automatically?"),
    el("p", {}, "Once a day the studio would ask GitHub whether a newer version is out. GitHub sees your IP address and the studio's " +
      "version, nothing about you, your files or your Tiiny. It never downloads or installs anything. You can change this " +
      "any time in Settings."),
    el("div", { class: "notice-row" },
      el("button", { class: "btn-sm accent", type: "button", onclick: () => answer(true) }, "Yes"),
      el("button", { class: "btn-sm", type: "button", onclick: () => answer(false) }, "No"),
      el("span", { class: "spacer" }),
      el("button", { class: "icon-btn", type: "button", title: "Ask me again next time", "aria-label": "Close without answering", onclick: () => note.remove() }, "✕")));
  $("#toasts").append(note);
}

async function checkUpdate() {
  let r;
  try { r = await api("/api/update"); } catch { return; }  // offline, or an older server: say nothing
  UPD.info = r?.available && isNumericVersion(String(r.version)) ? r : null;
  renderUpdatePill();
}

/** Shown while there is a newer release than the one dismissed (✕ remembers the version, not the day). */
function renderUpdatePill() {
  const i = UPD.info, pill = $("#updatePill");
  pill.hidden = !i || store.get("update.dismissed", "") === i.version;
  if (!pill.hidden) $("#updateOpen").textContent = `Tiiny Studio ${i.version} is available`;
}

function dismissUpdate() {
  if (UPD.info) store.set("update.dismissed", UPD.info.version);
  const inDialog = $("#updateModal").classList.contains("show");
  const fromPill = $("#updatePill").contains(document.activeElement);
  if (inDialog) closeUpdate();
  renderUpdatePill();
  if (fromPill) $("#settingsBtn").focus();  // the pill is gone; keep the keyboard user in the top bar
  announce("Update notice dismissed");
}

function openUpdate() {
  const i = UPD.info;
  if (!i) return;
  // the notes are GitHub's text: shown through the chat's Markdown renderer, which escapes everything first
  const notes = el("div", { class: "msg-md update-notes", tabindex: "0", role: "region", "aria-label": "Release notes" });
  if (typeof mdToHtml === "function") notes.innerHTML = mdToHtml(i.notes || "");
  else notes.textContent = i.notes || "";
  if (!notes.textContent.trim()) notes.textContent = "This release has no notes.";
  const link = safeLink(i.url);
  const card = el("div", { class: "modal-card set-card update-card" },
    el("div", { class: "modal-head" }, el("h3", { id: "updateTitle" }, `Tiiny Studio ${i.version} is available`), el("span", { class: "spacer" }),
      el("button", { class: "icon-btn", type: "button", title: "Close (Esc)", "aria-label": "Close", onclick: closeUpdate }, "✕")),
    el("div", { class: "set-body" }, notes),
    el("div", { class: "modal-foot" },
      link ? el("a", { class: "btn-sm accent", href: link, target: "_blank", rel: "noopener noreferrer" }, "Open the release page") : null,
      el("span", { class: "spacer" }),
      el("button", { class: "btn-sm ghost", type: "button", onclick: dismissUpdate }, "Dismiss until the next version"),
      el("button", { class: "btn-sm", type: "button", onclick: closeUpdate }, "Close")));
  $("#updateModal").replaceChildren(card);
  openModal($("#updateModal"));
}
function closeUpdate() { closeModal($("#updateModal")); }

/** The Settings switch. Stored on the server, which checks it before it ever contacts GitHub. */
function updateSettings() {
  const i = SET.info || {};
  const set = async (on) => {
    try { SET.info = await api("/api/settings/updates", { method: "POST", body: JSON.stringify({ update_check: on }) }); refreshSettings(); if (on) checkUpdate(); else { UPD.info = null; renderUpdatePill(); } }
    catch (e) { toast(e.message, "err"); refreshSettings(); }
  };
  return el("div", { class: "set-network set-swap" },
    el("b", {}, "Check for updates"),
    el("p", {}, "Once a day the studio asks GitHub whether a newer version is out, with one anonymous request that says nothing about you or your Tiiny, and shows a small notice if so. It never downloads or installs anything." + (i.docker ? " Off by default in Docker, where updates come with the image." : "")),
    el("label", { class: "auto" }, el("input", { type: "checkbox", checked: !!i.update_check, onchange: (e) => set(e.target.checked) }), "Check for updates"));
}

$("#updateOpen").onclick = openUpdate;
$("#updateDismiss").onclick = dismissUpdate;
document.addEventListener("keydown", (e) => { if (e.key === "Escape" && $("#updateModal").classList.contains("show")) { e.stopPropagation(); closeUpdate(); } }, true);
document.addEventListener("DOMContentLoaded", async () => { await askUpdateCheck(); checkUpdate(); });
