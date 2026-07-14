// Thin bridge over the injected Tauri globals (app.withGlobalTauri = true in
// tauri.conf.json, so no bundler is needed — window.__TAURI__.core.invoke /
// window.__TAURI__.event.listen per the v2 JS API reference).
//
// __TAURI__ may not exist yet when a top-level script runs (documented race:
// tauri-apps/tauri#12990), and never exists when a page is opened in a plain
// browser during design work — both cases degrade to a stub that resolves
// empty data instead of throwing.

"use strict";

const WSW = (() => {
  function tauri() {
    return typeof window !== "undefined" ? window.__TAURI__ : undefined;
  }

  async function ready(timeoutMs = 2000) {
    const t0 = Date.now();
    while (!tauri()) {
      if (Date.now() - t0 > timeoutMs) return false;
      await new Promise((r) => setTimeout(r, 25));
    }
    return true;
  }

  async function invoke(cmd, args) {
    if (!(await ready())) {
      console.warn(`[wsw] no Tauri runtime; '${cmd}' returns empty`);
      return null;
    }
    return tauri().core.invoke(cmd, args || {});
  }

  async function listen(event, handler) {
    if (!(await ready())) return () => {};
    return tauri().event.listen(event, handler);
  }

  function fmtTs(ms) {
    const s = Math.floor(ms / 1000);
    const m = Math.floor(s / 60);
    return `${String(m).padStart(2, "0")}:${String(s % 60).padStart(2, "0")}`;
  }

  function esc(text) {
    return String(text)
      .replaceAll("&", "&amp;")
      .replaceAll("<", "&lt;")
      .replaceAll(">", "&gt;")
      .replaceAll('"', "&quot;");
  }

  return { invoke, listen, fmtTs, esc };
})();
