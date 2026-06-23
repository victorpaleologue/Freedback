import { useEffect, useState } from "react";

// Light/dark theme toggle for the showcase. Shares the mechanics of the static
// pages' site/theme.js: the choice is persisted under localStorage "fb-theme"
// and reflected as data-theme on <html>; with no pinned choice the page follows
// the OS (prefers-color-scheme, handled in styles.css). The no-FOUC pre-paint
// apply lives inline in index.html — this component owns the runtime toggle.
const KEY = "fb-theme";

function systemDark() {
  return typeof window !== "undefined" && window.matchMedia
    ? window.matchMedia("(prefers-color-scheme: dark)").matches
    : false;
}

function stored() {
  try {
    return localStorage.getItem(KEY);
  } catch {
    return null;
  }
}

function effectiveTheme() {
  const s = stored();
  if (s === "dark" || s === "light") return s;
  return systemDark() ? "dark" : "light";
}

export default function ThemeToggle() {
  const [theme, setTheme] = useState(effectiveTheme);

  // Reflect the pinned choice onto <html> (or clear it to follow the system).
  useEffect(() => {
    const s = stored();
    const root = document.documentElement;
    if (s === "dark" || s === "light") root.setAttribute("data-theme", s);
    else root.removeAttribute("data-theme");
  }, [theme]);

  // When following the system (nothing pinned), live-update if the OS flips.
  useEffect(() => {
    if (!window.matchMedia) return undefined;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => {
      if (!stored()) setTheme(mq.matches ? "dark" : "light");
    };
    mq.addEventListener?.("change", onChange);
    return () => mq.removeEventListener?.("change", onChange);
  }, []);

  function toggle() {
    const next = effectiveTheme() === "dark" ? "light" : "dark";
    try {
      localStorage.setItem(KEY, next);
    } catch {
      /* ignore */
    }
    setTheme(next);
  }

  const dark = theme === "dark";
  return (
    <button
      type="button"
      className="fb-theme-toggle"
      data-fb-theme-toggle
      aria-pressed={dark}
      aria-label="Toggle light / dark theme"
      title={dark ? "Switch to light theme" : "Switch to dark theme"}
      onClick={toggle}
    >
      {dark ? "☀️" : "🌙"}
    </button>
  );
}
