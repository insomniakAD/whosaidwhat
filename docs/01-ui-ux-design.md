# Task 1 — UI/UX Design: whosaidwhat Dashboard

Evidence tiers used throughout (see BUILD_LOG.md D-008): **[fetched]** page retrieved
directly; **[search-verified]** substantial excerpts returned by the search engine,
primary URL cited, page itself blocked by this sandbox's egress proxy; **[inference]**
my design reasoning from those facts.

## 1. What the research shows

### 1.1 The category has converged on three layout families

| Product | Layout pattern | Source |
|---|---|---|
| Otter.ai | Left nav rail → conversation list → transcript front-and-center with Summary/Transcript tabs; right panel: Chat, Outline, Comments | [search-verified] https://help.otter.ai/hc/en-us/articles/21695002816535-Learn-about-the-new-Conversation-page-layout |
| Granola | User's own notepad is the primary surface; transcript hidden by default; post-meeting "Enhance notes" merges user bullets with transcript context; no meeting bot | [search-verified] https://zapier.com/blog/granola-ai/ |
| Notion AI Meeting Notes | A `/meeting` block with internal tabs (Notes \| Transcript) — Notion's first block with tabs; since Nov 2025 each summary takeaway carries a clickable citation deep-linking to the exact transcript moment | [search-verified] https://www.notion.com/help/ai-meeting-notes |
| superwhisper | Menu-bar app + dedicated recording window with live waveform; "mini window" variant reveals a stop button on hover | [search-verified] https://superwhisper.com/docs/get-started/interface-rec-window |
| Wispr Flow | Bottom-center pill that expands during dictation to Cancel / waveform / Done; so position-locked that a third-party utility (PillFloat) exists just to move it | [search-verified] https://github.com/OrangeAKA/pillfloat |
| Fathom | Video + transcript + sidebars; documented user complaint about *not* being able to hide the sidebars — evidence that always-on panels read as clutter | [search-verified] https://www.producthunt.com/products/fathom |
| tl;dv | Searchable meeting library; highlight transcript → auto-generate clip | [search-verified] https://tldv.io/features/meeting-recordings-transcriptions/ |
| Hyprnote (open-source, Tauri) | Live local transcript beside user notes; post-meeting enhancement via templates | [search-verified] https://github.com/fastrepl/hyprnote |
| Screenpipe (open-source, Tauri) | DVR-style scrubbing timeline of the day; drag a timeframe → ask AI about it | [search-verified] https://github.com/screenpipe/screenpipe |

Mobbin's public category taxonomy (Voice UI, Audio & Video Recorder, Recording Audio
& Video flows for mobile + web) confirms these same pattern families, but its free
tier (~4 apps) blocks deep teardown — the per-product sources above are the primary
evidence. [search-verified] https://mobbin.com/explore/mobile/screens/voice-ui

### 1.2 Platform context: macOS 26 "Tahoe" and Liquid Glass

- Liquid Glass shipped with macOS 26 (announced WWDC June 9 2025, released Sept 15
  2025). [search-verified] https://www.apple.com/newsroom/2025/06/apple-introduces-a-delightful-and-elegant-new-software-design/
- Apple's adoption guidance: glass is "best reserved for the navigation layer that
  floats above the content of your app" — sidebars and toolbars, never content.
  [search-verified] https://developer.apple.com/documentation/TechnologyOverviews/adopting-liquid-glass
- Reception was mixed (legibility complaints) and macOS 27 reportedly dials it back —
  restraint is the safe bet. [search-verified] https://macdailynews.com/2026/05/11/apple-to-refine-macos-27-with-liquid-glass-design-tweaks-after-macos-26-tahoe-backlash/

### 1.3 Color evidence

- Granola's 2025 rebrand (Ragged Edge): acid lime `#b5c832` accent on an off-white
  "near-paper" canvas; goal "calm, but with energy underneath". The strongest shipped
  example of a warm/light palette in exactly this category.
  [search-verified] https://abduzeedo.com/granola-brand-identity-design-ragged-edge
- 2026 trend reporting: warm sand, oatmeal, clay, taupe displacing pure white and cold
  darks in productivity tools; Pantone 2026 Color of the Year is "Cloud Dancer"
  (11-4201), a warm pale neutral. [search-verified] https://updivision.com/blog/post/ui-color-trends-to-watch-in-2026
- Otter uses "soft color contrasts" for speaker labels. [search-verified] (Otter help, above)
- Granola is repeatedly praised for feeling native and fast ("no electron wrapper
  sluggishness") — perceived native-ness is itself a differentiator.
  [search-verified] https://efficient.app/apps/granola

## 2. Design direction (my calls, labeled inference)

whosaidwhat's differentiator is in its name: **it knows who said what, on-device.**
The design should make speaker attribution the hero, something none of the shipped
apps foreground (Granola hides the transcript; Otter buries speakers in label chips).

### 2.1 Layout: three zones + one pill

```
┌──────────────────────────────────────────────────────────────────────┐
│ ⌘ toolbar (glass) — search everything · record button · settings     │
├───────────┬──────────────────────────────────────────────────────────┤
│           │  MEETING DETAIL                                          │
│ SIDEBAR   │  ┌────────────────────────────┬───────────────────────┐  │
│ (glass)   │  │ Notes  (primary pane)      │ Who-said-what rail    │  │
│           │  │                            │ (right, collapsible)  │  │
│ Today     │  │ AI notes with per-takeaway │                       │  │
│ ▸ Standup │  │ citations: click ¶ → jumps │ ● Me         14 min   │  │
│ Yesterday │  │ transcript + audio to ms   │ ● Sarah      11 min   │  │
│ ▸ 1:1 Kim │  │                            │ ● SPEAKER_02  3 min   │  │
│ ▸ Design  │  ├────────────────────────────┤ (click name → rename, │  │
│ July 10   │  │ Transcript (tab/collapsed) │  filter transcript)   │  │
│ ▸ ...     │  │ [02:14] Sarah: ...         │ + talk-time bars      │  │
│           │  └────────────────────────────┴───────────────────────┘  │
├───────────┴──────────────────────────────────────────────────────────┤
│ (during meetings only) floating pill: ● rec 12:41 ▁▃▅▃▁  [pause][■]  │
└──────────────────────────────────────────────────────────────────────┘
```

- **Sidebar** (glass, per Apple guidance): calendar-grouped meeting list — Today /
  Yesterday / date. Follows the universal sidebar+list+detail pattern (Otter, Notion,
  Hyprnote) so nothing needs explaining.
- **Notes-first detail pane** (Granola's proven bet, Notion's tab structure): the AI
  notes are the primary surface, the raw transcript one tab away. Every takeaway
  carries a citation chip (`[02:14]`) that deep-links to the transcript row *and*
  seeks the audio player — the Notion Nov-2025 pattern, nearly free for us because
  `segments` stores millisecond offsets (schema.sql).
- **Who-said-what rail** (the differentiator, inference): per-speaker talk-time bars,
  color-keyed dots reused in transcript speaker labels, click-to-rename (writes
  `speakers.display_name`), click-to-filter. Collapsible, answering Fathom users'
  documented clutter complaint.
- **Recording pill** (superwhisper/Wispr Flow convergence): small always-on-top
  window, live waveform, elapsed time, stop/pause on hover. **User-movable** — the
  existence of PillFloat is direct evidence that a locked pill frustrates. Doubles as
  the meeting-detected prompt ("Zoom meeting detected — ● Start recording") so consent
  and status share one surface (and it works in dev builds where notification action
  buttons don't — see docs/02).
- **Menu-bar item**: waveform glyph while recording; the app is useful with the main
  window closed (Granola/superwhisper pattern).

### 2.2 Color scheme: "Paper & Verdigris" (no navy, no slate, no cosmic black)

All values are original to this design (inference), anchored to the verified evidence
above (Granola's near-paper + single saturated accent formula; 2026 warm-neutral trend).

| Role | Light | Dark ("warm-dark", not black) |
|---|---|---|
| Canvas | `#FAF7F0` warm paper | `#2B2724` roasted umber |
| Raised panels / cards | `#FFFFFF` | `#353028` |
| Primary text | `#3D3A33` dark olive-brown | `#F0EBE0` |
| Secondary text | `#8A8474` | `#A89F8D` |
| **Accent — record & primary actions** | `#D96C47` terracotta | `#E8845F` |
| **Accent 2 — AI/summary surfaces** | `#3E7C6F` verdigris green | `#5FA293` |
| Speaker palette (dots, labels, waveforms) | 6-step warm categorical ramp: `#D96C47` `#3E7C6F` `#C9A227` `#8E5BA6` `#4E8FBF` `#B65A72` | same, +10% lightness |
| Success / recording-saved | `#5E8C4A` moss | `#7FA96B` |

Rationale (inference): terracotta reads "warm recording energy" without alarm-red
panic; verdigris separates "the machine wrote this" (summaries, AI chips) from "a
human said this" (transcript) at a glance; the six speaker hues stay distinguishable
against both canvases and echo Otter's soft-contrast speaker labels rather than
saturated tag colors. The dark theme is umber-based specifically because the prompt
forbids cosmic-black/slate dashboards — and the 2026 warm-neutral evidence says warm
darks are where the category is heading anyway.

Type: system stack (SF Pro / `-apple-system`) for UI; one display serif (New York)
for meeting titles — the serif-display-on-paper move is Granola's, and it signals
"notebook, not surveillance tool".

### 2.3 Motion & state (inference, sparingly)

- Live waveform in pill and transcript header (every recorder studied ships one —
  superwhisper, Wispr Flow; it is *the* "we can hear you" affordance).
- Transcript rows highlight as audio plays (Otter's karaoke pattern); citation chips
  pulse once when first rendered.
- Glass only on sidebar + toolbar (Apple guidance, §1.2); content panes are opaque
  paper — also keeps the Tauri webview honest, since real Liquid Glass is
  native-only and a faithful web imitation would violate the restraint guidance.

## 3. What got cut / open questions

- Mobbin Pro teardown (paywalled; free tier inadequate — evidence above).
- Mobile companion design: out of scope for a macOS-first dashboard; the layout
  collapses to list→detail with the rail becoming a bottom sheet if ever needed.
- Live in-meeting transcript pane (Hyprnote has one): deferred — v1 pipeline is
  post-meeting batch (see docs/00-architecture.md); the pill communicates recording
  state without implying live transcription we don't do yet.
