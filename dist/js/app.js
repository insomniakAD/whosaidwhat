// Dashboard logic. Layout and interactions follow docs/01 §2.1: sidebar
// meeting list grouped by day → notes-first detail pane with per-takeaway
// [mm:ss] citation chips → collapsible who-said-what rail (talk-time bars,
// click-to-filter, double-click-to-rename) → action items under the notes.
//
// All markup is built from escaped text (WSW.esc) — transcript and summary
// content is model/user data and must never reach innerHTML raw.

"use strict";

(async () => {
  const $ = (id) => document.getElementById(id);
  const state = {
    meetings: [],
    currentId: null,
    tab: "notes", // notes | transcript | outline | search
    segments: [],
    summaries: {}, // kind -> SummaryRow|null
    actionItems: [],
    stats: [],
    speakerFilter: null, // speaker_id
    recording: false,
    progress: null, // {meeting_id, stage, done, total}
  };

  const speakerColor = (() => {
    const assigned = new Map();
    return (speakerId) => {
      if (!assigned.has(speakerId)) assigned.set(speakerId, assigned.size % 6);
      return `var(--sp-${assigned.get(speakerId)})`;
    };
  })();

  // ---------- markdown-mini: headings, bullets, bold; everything escaped ----------

  function inline(text) {
    let html = WSW.esc(text);
    html = html.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
    // [mm:ss] / [h:mm:ss] markers become citation chips (the Notion pattern,
    // docs/01 §2.1); same accepted forms as llm::extract on the Rust side.
    html = html.replace(/\[(\d{1,4}:\d{2}(?::\d{2})?)\]/g, (m, ts) => {
      const ms = tsToMs(ts);
      return ms == null ? m : `<button class="cite" data-ms="${ms}">${ts}</button>`;
    });
    return html;
  }

  function tsToMs(ts) {
    const parts = ts.split(":").map(Number);
    if (parts.some(Number.isNaN)) return null;
    if (parts.length === 2) return parts[1] < 60 ? (parts[0] * 60 + parts[1]) * 1000 : null;
    if (parts.length === 3 && parts[1] < 60 && parts[2] < 60)
      return ((parts[0] * 60 + parts[1]) * 60 + parts[2]) * 1000;
    return null;
  }

  function renderMarkdown(md) {
    const out = [];
    let inList = false;
    for (const raw of md.split("\n")) {
      const line = raw.trim();
      const bullet = /^[-*•]\s+(.*)/.exec(line);
      if (bullet) {
        if (!inList) { out.push("<ul>"); inList = true; }
        out.push(`<li>${inline(bullet[1])}</li>`);
        continue;
      }
      if (inList) { out.push("</ul>"); inList = false; }
      if (!line) continue;
      const h = /^(#{1,3})\s+(.*)/.exec(line);
      if (h) out.push(`<h${h[1].length + 1}>${inline(h[2])}</h${h[1].length + 1}>`);
      else out.push(`<p>${inline(line)}</p>`);
    }
    if (inList) out.push("</ul>");
    return out.join("\n");
  }

  // ---------- sidebar ----------

  function dayLabel(epochS) {
    const d = new Date(epochS * 1000);
    const today = new Date();
    const yest = new Date(today.getTime() - 86400e3);
    const same = (a, b) => a.toDateString() === b.toDateString();
    if (same(d, today)) return "Today";
    if (same(d, yest)) return "Yesterday";
    return d.toLocaleDateString(undefined, { month: "long", day: "numeric" });
  }

  function renderSidebar() {
    const nav = $("sidebar");
    nav.innerHTML = "";
    if (!state.meetings.length) {
      nav.innerHTML = `<div class="empty">No meetings yet.<br />Join one and say yes.</div>`;
      return;
    }
    let lastDay = null;
    for (const m of state.meetings) {
      const day = dayLabel(m.started_at);
      if (day !== lastDay) {
        lastDay = day;
        const label = document.createElement("div");
        label.className = "day-label";
        label.textContent = day;
        nav.appendChild(label);
      }
      const btn = document.createElement("button");
      btn.className = "meeting-item" + (m.id === state.currentId ? " active" : "");
      const time = new Date(m.started_at * 1000).toLocaleTimeString(undefined, {
        hour: "2-digit", minute: "2-digit",
      });
      const status = m.status.startsWith("failed")
        ? `<span class="status-failed">${WSW.esc(m.status)}</span>`
        : m.status === "summarized" ? "" : ` · ${WSW.esc(m.status)}`;
      btn.innerHTML = `<span class="t">${WSW.esc(m.title)}</span>` +
        `<span class="m">${time}${m.app ? " · " + WSW.esc(m.app) : ""}${status}</span>`;
      btn.addEventListener("click", () => openMeeting(m.id));
      nav.appendChild(btn);
    }
    if (state.progress) {
      const p = document.createElement("div");
      p.className = "progress-line";
      p.textContent = `processing: ${state.progress.stage} ${state.progress.done}/${state.progress.total}`;
      nav.appendChild(p);
    }
  }

  // ---------- detail ----------

  async function openMeeting(id) {
    state.currentId = id;
    state.speakerFilter = null;
    if (state.tab === "search") state.tab = "notes";
    const [segments, notes, outline, actionItems, stats] = await Promise.all([
      WSW.invoke("get_segments", { meetingId: id }),
      WSW.invoke("get_summary", { meetingId: id, kind: "notes" }),
      WSW.invoke("get_summary", { meetingId: id, kind: "outline" }),
      WSW.invoke("get_action_items", { meetingId: id }),
      WSW.invoke("get_speaker_stats", { meetingId: id }),
    ]);
    state.segments = segments || [];
    state.summaries = { notes, outline };
    state.actionItems = actionItems || [];
    state.stats = stats || [];
    renderSidebar();
    renderDetail();
    renderRail();
  }

  function currentMeeting() {
    return state.meetings.find((m) => m.id === state.currentId) || null;
  }

  function renderDetail() {
    const el = $("content");
    const m = currentMeeting();
    if (!m) {
      el.innerHTML = `<div class="empty">Select a meeting — or just start talking.</div>`;
      return;
    }
    const started = new Date(m.started_at * 1000).toLocaleString();
    const summary = state.summaries.notes;
    const model = summary
      ? `<span class="model-chip">${WSW.esc(summary.model)}${summary.model_was_fallback ? " (fallback)" : ""}</span>`
      : "";
    const tabs = ["notes", "transcript", "outline"]
      .map((t) => `<button class="tab${state.tab === t ? " active" : ""}" data-tab="${t}">` +
        `${t[0].toUpperCase()}${t.slice(1)}</button>`)
      .join("");

    let body = "";
    if (state.tab === "notes") body = renderNotes();
    else if (state.tab === "outline") body = renderOutline();
    else body = renderTranscript();

    el.innerHTML =
      `<h1 class="meeting-title">${WSW.esc(m.title)}</h1>` +
      `<div class="meeting-sub">${WSW.esc(started)} ${model}</div>` +
      `<div class="tabs">${tabs}</div>` + body + renderActions();

    el.querySelectorAll(".tab").forEach((t) =>
      t.addEventListener("click", () => { state.tab = t.dataset.tab; renderDetail(); }));
    el.querySelectorAll(".cite").forEach((c) =>
      c.addEventListener("click", () => jumpToMs(Number(c.dataset.ms))));
    el.querySelectorAll(".action input").forEach((cb) =>
      cb.addEventListener("change", async () => {
        await WSW.invoke("set_action_item_done", { id: Number(cb.dataset.id), done: cb.checked });
        cb.closest(".action").classList.toggle("done", cb.checked);
      }));
  }

  function renderNotes() {
    const s = state.summaries.notes;
    if (!s) return `<div class="empty">No summary yet — the pipeline runs right after a recording ends.</div>`;
    return `<div class="ai-note">AI notes — citations click through to the transcript</div>` +
      `<div class="notes">${renderMarkdown(s.content)}</div>`;
  }

  function renderOutline() {
    const s = state.summaries.outline;
    if (!s) return `<div class="empty">No outline yet.</div>`;
    return `<div class="notes">${renderMarkdown(s.content)}</div>`;
  }

  function renderTranscript() {
    if (!state.segments.length) return `<div class="empty">No transcript yet.</div>`;
    const rows = state.segments
      .filter((s) => state.speakerFilter == null || s.speaker_id === state.speakerFilter)
      .map((s) =>
        `<div class="turn${s.source === "mic" ? " mic-track" : ""}" data-seg="${s.id}" data-start="${s.start_ms}">` +
        `<span class="sp-dot" style="background:${speakerColor(s.speaker_id)}"></span>` +
        `<span class="ts">${WSW.fmtTs(s.start_ms)}</span>` +
        `<span><span class="who">${WSW.esc(s.speaker)}</span>${WSW.esc(s.text)}</span></div>`)
      .join("");
    return `<div class="transcript">${rows}</div>`;
  }

  function renderActions() {
    if (!state.actionItems.length) return "";
    const rows = state.actionItems.map((a) =>
      `<div class="action${a.done ? " done" : ""}">` +
      `<input type="checkbox" data-id="${a.id}"${a.done ? " checked" : ""} />` +
      `<span class="txt">${a.owner ? `<span class="owner">${WSW.esc(a.owner)}</span> — ` : ""}` +
      `${WSW.esc(a.text)}</span></div>`).join("");
    return `<div class="actions-block"><h2>Action items</h2>${rows}</div>`;
  }

  function jumpToMs(ms) {
    state.tab = "transcript";
    state.speakerFilter = null;
    renderDetail();
    // Nearest row at/before the cited moment (rows are start-sorted).
    const rows = [...document.querySelectorAll(".turn")];
    let target = rows[0];
    for (const r of rows) {
      if (Number(r.dataset.start) <= ms) target = r;
      else break;
    }
    if (target) {
      target.scrollIntoView({ behavior: "smooth", block: "center" });
      target.classList.add("flash");
      setTimeout(() => target.classList.remove("flash"), 1600);
    }
  }

  // ---------- who-said-what rail ----------

  function renderRail() {
    const list = $("rail-list");
    list.innerHTML = "";
    if (!state.stats.length) {
      list.innerHTML = `<div class="hint">Speakers appear after transcription.</div>`;
      return;
    }
    const maxMs = Math.max(...state.stats.map((s) => s.talk_ms), 1);
    for (const s of state.stats) {
      const row = document.createElement("button");
      row.className = "speaker-row" + (state.speakerFilter === s.speaker_id ? " filtered" : "");
      const mins = Math.round(s.talk_ms / 60000);
      row.innerHTML =
        `<span class="line"><span class="sp-dot" style="background:${speakerColor(s.speaker_id)}"></span>` +
        `<span class="name">${WSW.esc(s.display_name)}${s.is_self ? " (you)" : ""}</span>` +
        `<span class="mins">${mins} min</span></span>` +
        `<span class="talk-bar" style="width:${(100 * s.talk_ms / maxMs).toFixed(0)}%;` +
        `background:${speakerColor(s.speaker_id)}"></span>`;
      row.addEventListener("click", () => {
        state.speakerFilter = state.speakerFilter === s.speaker_id ? null : s.speaker_id;
        state.tab = "transcript";
        renderDetail();
        renderRail();
      });
      // Inline rename (window.prompt is not reliably available in wry
      // webviews, so the name span becomes an input in place).
      row.addEventListener("dblclick", () => {
        const nameEl = row.querySelector(".name");
        const input = document.createElement("input");
        input.className = "search";
        input.value = s.display_name;
        nameEl.replaceWith(input);
        input.focus();
        input.select();
        const commit = async () => {
          const name = input.value.trim();
          if (name && name !== s.display_name) {
            await WSW.invoke("rename_speaker", { speakerId: s.speaker_id, name });
          }
          await openMeeting(state.currentId);
        };
        input.addEventListener("keydown", (ev) => {
          if (ev.key === "Enter") commit();
          if (ev.key === "Escape") renderRail();
        });
        input.addEventListener("blur", () => renderRail());
      });
      list.appendChild(row);
    }
  }

  // ---------- search ----------

  let searchTimer = null;
  $("search").addEventListener("input", (e) => {
    clearTimeout(searchTimer);
    const q = e.target.value.trim();
    searchTimer = setTimeout(() => runSearch(q), 200);
  });

  async function runSearch(q) {
    if (!q) {
      state.tab = "notes";
      renderDetail();
      return;
    }
    const hits = (await WSW.invoke("search_transcripts", { query: q })) || [];
    const el = $("content");
    const byId = new Map(state.meetings.map((m) => [m.id, m]));
    const rows = hits.map((h, i) => {
      const m = byId.get(h.meeting_id);
      // FTS5 snippet marks matches with [ ]; swap for <b> after escaping.
      const snip = WSW.esc(h.snippet).replaceAll("[", "<b>").replaceAll("]", "</b>");
      return `<button class="result" data-i="${i}">` +
        `<span class="meta">${WSW.esc(m ? m.title : h.meeting_id)}` +
        `${h.speaker ? " · " + WSW.esc(h.speaker) : ""} · ${WSW.fmtTs(h.start_ms)}</span><br />` +
        `${snip}</button>`;
    }).join("");
    state.tab = "search";
    el.innerHTML = `<h1 class="meeting-title">“${WSW.esc(q)}”</h1>` +
      `<div class="meeting-sub">${hits.length} moments</div>` +
      `<div class="results">${rows || `<div class="empty">Nothing said about that.</div>`}</div>`;
    el.querySelectorAll(".result").forEach((r) =>
      r.addEventListener("click", async () => {
        const hit = hits[Number(r.dataset.i)];
        await openMeeting(hit.meeting_id);
        jumpToMs(hit.start_ms);
      }));
  }

  // ---------- record button + events ----------

  $("record-btn").addEventListener("click", () => {
    WSW.invoke(state.recording ? "stop_recording" : "start_manual_recording");
  });

  function setRecording(rec) {
    state.recording = rec;
    $("record-btn").classList.toggle("recording", rec);
    $("record-label").textContent = rec ? "Recording — stop" : "Record";
  }

  async function refreshMeetings() {
    state.meetings = (await WSW.invoke("list_meetings", {})) || [];
    renderSidebar();
  }

  await WSW.listen("recording-state", (e) => setRecording(!!(e.payload && e.payload.recording)));
  await WSW.listen("meeting-saved", () => refreshMeetings());
  await WSW.listen("pipeline-progress", (e) => { state.progress = e.payload; renderSidebar(); });
  await WSW.listen("summary-ready", async (e) => {
    state.progress = null;
    await refreshMeetings();
    if (e.payload && e.payload.meeting_id === state.currentId) openMeeting(state.currentId);
    else if (!state.currentId && e.payload) openMeeting(e.payload.meeting_id);
  });
  await WSW.listen("pipeline-failed", () => { state.progress = null; refreshMeetings(); });

  // ---------- boot ----------

  const status = await WSW.invoke("get_status", {});
  if (status) setRecording(!!status.recording);
  await refreshMeetings();
  if (state.meetings.length) openMeeting(state.meetings[0].id);
})();
