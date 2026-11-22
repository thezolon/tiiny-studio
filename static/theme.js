// The saved theme (Settings > Appearance), applied before first paint; a file, not inline, so the CSP can forbid inline scripts.
try { var t = JSON.parse(localStorage.getItem("tiiny.theme")); if (t === "light" || t === "dark") document.documentElement.dataset.theme = t; } catch (e) {}
