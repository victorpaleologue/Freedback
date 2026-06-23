/* Freedback theme switch — shared by every static page.
 *
 * Precedence: an explicit choice (the toggle, persisted in localStorage) wins;
 * otherwise the page follows the OS/browser prefers-color-scheme. The dark
 * palette itself lives in freedback.css; this script only sets/clears
 * data-theme on <html> and wires up the toggle button(s).
 *
 * Load it as a CLASSIC script in <head> (before the body paints) so the pinned
 * theme is applied with no flash of the wrong colors. Mark a button with
 * `data-fb-theme-toggle` and (optionally) put the icon in a child element with
 * `data-fb-theme-icon`. */
(function () {
  var KEY = "fb-theme";
  var root = document.documentElement;

  function stored() {
    try { return localStorage.getItem(KEY); } catch (e) { return null; }
  }
  function systemDark() {
    return !!(window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)").matches);
  }
  function effective() {
    var s = stored();
    if (s === "dark" || s === "light") return s;
    return systemDark() ? "dark" : "light";
  }

  // Reflect the stored choice onto <html>. No stored choice => remove the
  // attribute so the CSS prefers-color-scheme rule takes over (follow system).
  function applyAttr() {
    var s = stored();
    if (s === "dark" || s === "light") root.setAttribute("data-theme", s);
    else root.removeAttribute("data-theme");
  }

  function updateButtons() {
    var eff = effective();
    var btns = document.querySelectorAll("[data-fb-theme-toggle]");
    for (var i = 0; i < btns.length; i++) {
      var b = btns[i];
      b.setAttribute("aria-pressed", eff === "dark" ? "true" : "false");
      var icon = b.querySelector("[data-fb-theme-icon]") || b;
      icon.textContent = eff === "dark" ? "☀️" /* ☀️ */ : "🌙" /* 🌙 */;
      b.title = eff === "dark" ? "Switch to light theme" : "Switch to dark theme";
    }
  }

  function toggle() {
    var next = effective() === "dark" ? "light" : "dark";
    try { localStorage.setItem(KEY, next); } catch (e) {}
    applyAttr();
    updateButtons();
  }

  // Apply ASAP (head time, before paint). Buttons don't exist yet — wired below.
  applyAttr();

  document.addEventListener("DOMContentLoaded", function () {
    var btns = document.querySelectorAll("[data-fb-theme-toggle]");
    for (var i = 0; i < btns.length; i++) btns[i].addEventListener("click", toggle);
    updateButtons();
  });

  // When following the system (no pinned choice), live-update if the OS flips.
  if (window.matchMedia) {
    try {
      window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", function () {
        if (!stored()) updateButtons();
      });
    } catch (e) { /* older Safari: ignore */ }
  }

  window.FreedbackTheme = { toggle: toggle, effective: effective };
})();
