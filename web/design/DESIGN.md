# h2 live — After-hours uplink

A community game terminal with the lights still on. Graphite and blue-grey panels hold the records; green light means someone is here, something is live, or there is somewhere to go. The emotional promise is **“The game goes on.”** Dense information, familiar names, and the pleasure of opening a report after a close finish.

This is a visual proposal for the SolidJS engineer, not a replacement application. `home.html`, `game.html`, `players.html`, `player.html` and `events.html` run independently, share `tokens.css` / `mockup.css`, and link to one another. `mockup.js` provides local filtering, server-selection feedback, and a replay clock preview. There is no service connection, authentication, real verification, or persisted preference. All names, records and geometry are fictional samples. Counts are illustrative; the directory and recent-game tables are excerpts.

## Reference eras, original execution

- **Bungie.net’s Halo 2-era community stats pages (mid-2000s):** compact tab navigation, inset tables, disciplined small uppercase labels, strong data hierarchy, player service records and postgame “carnage reports.” The composition is newly designed: a wide welcoming overview, a tactical two-column game page, and a separate inspectable event stream. No archived page was traced or used as a layout template.
- **The original Xbox Live dashboard (2002–2005):** a warm green connection light, jewel-like action buttons, subtle translucent curves and the sense of a network waiting after dark. The original `signal-lens.svg` is an offset set of ellipses and a flowing band, deliberately without a branded centre, glyph or console silhouette. It is a static decorative instrument, not a recreation of a boot animation.
- **Present-day readability:** contrast-tested type, real table semantics, self-hosted fonts, named controls, visible keyboard focus, honest stream and signature states, and a single-column phone composition. Nostalgia belongs in the materials; data stays crisp.

The app name is plain typographic **h2 live**. Do not add platform logos, game emblems, rank badges, screenshots, weapon icons, retail map silhouettes, sounds, or proprietary typography. Initial tiles are the only player art. Both SVG files are original vector constructions for this study. `map-schematic.svg` is fictional geometry, not an outline of a game level. A deployed app may render an outline received from its selected server; do not bundle retail geometry with the public source.

## Foundations / tokens

All reusable tokens have the `--h2-` prefix in `tokens.css`. Import it before the application stylesheet. Avoid globally adopting mockup CSS class names without scoping them: this is a visual reference, not a framework dependency.

### Colour and contrast

Ratios below use WCAG relative sRGB luminance, rounded to two decimals. All text tokens exceed **4.5:1** even on the lightest standard content surface (`#1a2934`). Use them at full opacity. Do not dim labels by reducing opacity. Decorative separators are not text and are intentionally quieter.

| Purpose / token suffix | Hex | On base `#090f14` | On panel `#111c25` | On raised `#1a2934` |
| --- | --- | ---: | ---: | ---: |
| `text` | `#e5edf2` | 16.26 | 14.57 | 12.57 |
| `text-secondary` | `#b2c3ce` | 10.63 | 9.53 | 8.22 |
| `text-muted` | `#91a5b3` | 7.56 | 6.77 | 5.84 |
| `green` — live / action | `#b4f477` | 14.81 | 13.27 | 11.45 |
| `cyan` — links / inspected data | `#8dd6ef` | 11.92 | 10.68 | 9.21 |
| `amber` — unknown / interrupted | `#f2cc80` | 12.59 | 11.28 | 9.73 |
| `red` — coral team / failure | `#ffa394` | 10.02 | 8.97 | 7.74 |
| `blue` — blue team | `#8acbff` | 11.07 | 9.92 | 8.55 |
| `purple` — additional team | `#d6b4ff` | 10.85 | 9.72 | 8.39 |

Backgrounds: `bg #090f14`; `surface-inset #0c151c`; `surface #111c25`; `surface-raised #1a2934`. Panel line `border #354854`; essential input/control outline `border-strong #718795` (3.97:1 against raised). Bevel highlight `#526471`; bevel shadow `#05090d`.

Primary buttons use `green-ink #122208` on the green jewel gradient, with **9.93:1** at its darkest stop (`#9cd95c`). Text never sits over transparent geometry without an opaque or sufficiently dark scrim. The faint hero coordinates and map grid are decorative; meaningful map labels use opaque light text. Green is not a team colour in the first two-team scheme, so live controls do not compete with the teams. Colour is always paired with text, shape, or state icon.

### Typography and licensing

Self-hosted, unmodified TTF files are included under `fonts/`. Retain the associated copyright and licence files when redistributing. No third-party font requests occur at runtime.

| Font | Use | Weights included | Licence / primary source |
| --- | --- | --- | --- |
| Barlow | Body, nav, controls, labels, gamertags | 400, 600, 700 | SIL Open Font License 1.1; [Google Fonts source](https://github.com/google/fonts/tree/main/ofl/barlow); `fonts/barlow-OFL.txt` |
| Barlow Condensed | Page titles, panel section headings, large metric figures | 600 | SIL Open Font License 1.1; [Google Fonts source](https://github.com/google/fonts/tree/main/ofl/barlowcondensed); `fonts/barlowcondensed-OFL.txt` |
| IBM Plex Mono | Times, scores, positions, keys, identity strings | 400 | SIL Open Font License 1.1; [Google Fonts source](https://github.com/google/fonts/tree/main/ofl/ibmplexmono); `fonts/ibmplexmono-OFL.txt` |

Use `font-display: swap`. The fallback stacks in tokens avoid proprietary font distribution. Production can convert to WOFF2 or subset with the same licence notices; do not make a font CDN a dependency for connecting to a community server.

Scale: 12 / 14 / 16 / 20 / 24 / 32 / 44 px. Hero: `clamp(44px, 5vw, 72px)`; phone hero 50 px, page title 38 px. Body leading 1.5; display leading 1.05–1.1. Uppercase labels use 0.13 em tracking and 600 weight, not a simulated distressed typeface. 10–11 px is reserved for ancillary metadata, never instructions or primary form labels in production; increase control labels to 12 px where needed. Numbers are tabular, right aligned in tables. Allow gamertags to wrap and never silently truncate the identity on the profile or inspector.

### Geometry, finish, motion

- Spacing: 4 / 8 / 12 / 16 / 20 / 24 / 32 / 40 / 48 px. Page max-width 1280 px. Desktop outer gutter 32 px (expands naturally beyond max-width), phone 14 px. Desktop section gap 24–30 px; panel padding 16–20 px; hero 34–40 px.
- Radii: labels 2 px, panels 4 px, jewel buttons 6 px, dots fully round. No pill-shaped navigation.
- Borders: 1 px structural lines. A light top edge and dark bottom edge suggest machined metal. Use a 3 px green bottom bar for the active navigation item and a 3 px left bar for selected table rows; never rely on fill alone.
- Metal: `#263844 → #192833 → #121e28`, vertical. Primary jewel: `#d4ff9f → #b4f477 → #9cd95c`. Shadows are tight: 0 6px 18px at 18% black plus a 5% white inner top edge. Inset inputs get 0 2px 5px at 45% black.
- Glow: green actions 20 px blur at 14%; connection dot 8 px at 50%. Do not glow body text, every border, team tables, or the entire viewport.
- Texture: CSS scanlines, 1 px every 4 px at 10% black, confined to the decorative hero. Map grid is 32 px with faint blue lines. SVG hatching separates walkways and lower geometry. No photographic noise or full-screen filter; avoid runtime `feTurbulence` and animated filters.
- Motion: feedback 120 ms, panel/control transitions 180 ms, optional one-time page reveal 240 ms. Easing `cubic-bezier(.2,.8,.2,1)`. Live-map interpolation budget is about 67 ms for a 15 Hz stream. No endlessly rotating ornament, flashing “live” label, or boot delay. A glow is static.
- Z layers: base 0, ornaments 1, content 2, sticky headers 10, popovers 30, dialogs 50, notices/toasts 60. Tooltips must not appear under a scroll container or cover their anchor's focus ring.

### Reduced motion

`prefers-reduced-motion: reduce` zeroes transition tokens and disables decorative animation and smooth scrolling. Snap player markers to frames instead of interpolating. Replay remains available with explicit user-controlled play/pause; do not auto-play it on entry. Avoid animated skeletons. Do not announce or flash every frame. The static mockups have no automatic motion in either mode.

## Component specifications

### Top bar and server switcher

Desktop: 90 px masthead with plain text brand, a short community line, the current server control, and a 42–44 px initial tile linking to a service record. Phone: brand and tile first, full-width server control below. Keep the current server visible on every page, including event details and profiles.

Switcher summary: status dot + text state, discovered server ID (full value available to assistive technology), chevron. Open a 340 px popover on desktop; use the available width on phone. Include a labelled **Server URL** field, discovery action, cancel/close control in production, and inline errors. The HTML uses native `<details>` and Escape support; a production dialog/popover must implement focus return and outside dismissal. Never dismiss it while a person is editing because a stream event arrived.

Flow: enter base URL → fetch `/.well-known/h2live.json` → show discovered ID and API endpoint plus connection status → apply selection. URL input should preserve subpaths and use the discovered API URL; do not concatenate `/v1` blindly. Keep the old server active if discovery fails. On successful switch, close old sockets, cancel outstanding reads, clear server-scoped caches/selection, and load the new overview. Namespace identity by home server; a matching gamertag on another server is not the same account. Explain that history and account names belong to the chosen server. Do not claim federation, which the discovery example marks false. A green connection indicator means reachable/caught up, not operator trust. The design's form explicitly previews without making a connection.

### Navigation

Overview / Games / Players / Event log are page links in a `<nav>` with `aria-current="page"`; they are not ARIA tabs. One horizontal row, active green underline, equal width on phone. Players remains active on an individual profile. Game detail includes a breadcrumb. Live / Replay inside the game are buttons with `aria-pressed`; if implemented as ARIA tabs, add the full keyboard/tab-panel semantics.

### Panels and game cards

Panels: 1 px border, 4 px radius, clipped header bevel, plain content surface. Header is 46 px minimum; uppercase title on left, count or status on right. Footers carry source and secondary actions. Never make low-contrast panel dividers the sole boundary of an interactive control.

Game card: 155–170 px decorative mini-map; live badge at top left; elapsed time top right; variant label, large map name, player count and team mode; team score strip or FFA leader; full-width “Watch game” footer. Main link goes to game ID, not map name. Mini-map is not interactive and is hidden from assistive technology in production if its information is redundant. Missing outline: use a blank grid with “Map outline not available”; keep scores and Watch game. Do not silently substitute the demo arena as if it were real geometry.

### Live map / recorded map

- Inset dark canvas with supplied floor polygons, faint grid, neutral strokes and subtle hatch patterns. Preserve aspect ratio; fit to server bounds with 24 px padding. Do not crop players to fill a card.
- Convert world x/y through a consistent transform; account for screen y direction and the engine's yaw convention. Keep z in the data and filter by the outline's height ranges. The floor select lists actual available layers; empty layers are disabled with a reason.
- Slot 01 etc. is `index + 1` for display, keyed by the stable game `index`; retain the zero-based index in event inspection. Circle = coral team, diamond = blue. A short triangle outside the marker shows yaw. Dot/diamond body 7–8 SVG units, dark 2 px outline, text slot label. Use at least 8 screen px after fitting; touch selection should have an invisible 44 px hit region. Ring the selected player in white. A white × means dead; don't leave both a living dot and a death marker at that slot.
- Coral `#ffa394`, blue `#8acbff`; additional teams may use purple/square, amber/triangle, cyan/hexagon. Retain team number/name and a labelled legend for all teams. Team index → appearance mapping is deterministic within a game; do not assume team 0/1 universally. For FFA, use neutral cyan circles with slot labels, and green for the selected player; no invented teams. Slot and shape provide alternatives to hue.
- Movement arrives at up to 15 Hz; animate marker transforms only, not table rows or the entire DOM. Buffer/interpolate without assuming exact cadence. Freeze after a stale threshold (proposed 2 seconds without expected activity, tuned with actual stream behaviour); label “Positions delayed · last frame …”. A quiet event log alone is not evidence of a disconnect.
- Recorded frames are 1 Hz. Replay scrubber maps to `tick / 30`, requests only the needed range, handles gaps explicitly, and labels approximate interpolation if used. Never suggest 15 Hz fidelity in a replay. A selected range may be loading; retain last complete frame and show the requested time separately.
- The demonstration has one static schematic. Its floor select previews the label and de-emphasizes the drawing; it does not implement geometric layer filtering. Its replay changes only the clock. The engineer must bind real frame and layer data.

### Scoreboards: team and FFA

Use semantic tables with a caption and scoped column headers. Column order: Player / Kills / Deaths / Assists / Score. Long names wrap while numeric columns stay aligned. Mono for numbers; 13–14 px gamertags; 12 px numbers. Expand K / D / A with accessible labels or abbreviations. A selected player has a green left edge, a subtle tint, and an explicit selected state for assistive technology.

Team table: group rows under “● Coral team — 32” and “◆ Blue team — 28”; show explicit team scores from the server. Do not calculate objective score by adding kills. Sort team scores descending, then players by score with a deterministic tie-break. Winner is only shown after `GameEnded` supplies a meaningful winner.

FFA table: no team header; add a **Place** column before Player. Example:

| Place | Player | K | D | A | Score |
| ---: | --- | ---: | ---: | ---: | ---: |
| 1 | Glass Radio | 18 | 10 | 2 | 18 |
| 2 | Mint Condition | 16 | 12 | 4 | 16 |
| 3 | Drift Signal | 12 | 14 | 1 | 12 |

Tie scores display tied place; use slot order only as a stable rendering order, not to invent a winner. Null/absent stats show an em dash, not 0. For campaign, use the available roster/stats and omit competitive rank/winner UI when unsupported. Disconnected/leaving players remain in the report with a “Left” tag; don't erase their score.

### Kill feed

Compact chronological rows, latest visible first: game time, killer name, textual action, victim name, optional weapon text. Team colour and slot labels match the map. Do not reproduce weapon silhouettes or proprietary medals. `kind=kill`: “A eliminated B”; `suicide`: “A self-eliminated”; `betrayal`: “A betrayed B”; `world`: “A died (world).” Do not infer a more specific cause from the world category. The mockup uses a fictional world-death example. Unknown killer or weapon should be omitted or say “Unknown,” never guessed. Rows preserve the selected replay time: do not show future kills while scrubbing. Announce batches or counts in a polite region, never every 15 Hz frame.

### Player / service-record card

Initial tile, gamertag, text presence, home server, compact stats, service-record link. Do not invent a logo/avatar or official rank. Profile hero adds full `gamertag@home`, join date, and current-game link. An unlinked local player is shown by name and game slot; no fabricated XUID or profile route.

Game count, K/D, win rate, variant totals and weekly chart are **derived proposals**, not fields currently guaranteed by `/v1/accounts`. Display them only when the application has a complete supported aggregation, with scope and coverage. If it has only `recent_games`, label **Recent recorded games**, or omit totals; never present a partial sum as lifetime stats. With zero deaths show kills and deaths and “No deaths recorded” instead of an infinite ratio. No completed games: “Your first chapter is still ahead.” Unknown wins or unsupported modes are excluded with a stated denominator, not counted as losses. Format a percent after computing the underlying fraction; 82 / 128 rounds to 64%.

### Presence

Online: filled green dot + “Online”; in game: same dot + “In game · [map]”; away if actually supplied: amber dot + “Away”; offline: hollow muted dot + “Offline” / last seen if known. Small dots supplement text. Presence and account identity are separate. Expired presence is not currently online. Do not synthesize friends: “Familiar signals” is a sample online list, not an implemented friendship relationship. If there is no friend API, use “Online players” in production.

### Signature marks

Use original text glyphs, not shields or branded ticks: **✓ Signature verified** in cyan; **◇ Unverified** in amber; **! Invalid signature** in coral. Loading verification: “Checking signature…”. Always include the full words or an accessible name; hover-only explanations are insufficient.

Verification means the **JWS signed body** was checked against its author's applicable key, not that the author is trusted or the gameplay claim is true. Do not apply an event verification tick to an entire player profile, unsigned live frame, connection, or game history based on one checked record. Preserve invalid/unknown records in the log with an explicit reason and allow inspection. Unknown `kid` may trigger the contract's key fetch, then resolve to verified, invalid, or unable to verify. The inspector should display the decoded signed fields as authoritative; copied envelope fields are convenience data. The mockup intentionally uses sample status, not valid JWS material.

### Buttons, inputs and focus

Primary: green jewel, dark text, 44 px minimum height. Secondary: metallic fill, light text, strong border. Quiet: dark fill, visible focus, text link treatment. One prominent primary action per local task. No animated glare. Disabled buttons keep a textual reason nearby; never disable the only route out of an error. Hover brightens slightly, pressed darkens subtly, focus is 2 px green with 3 px offset. Visible labels are required for every field; search placeholders are hints, not labels. Errors attach via `aria-describedby`, mark invalid fields and preserve input. Use native URL, search, range and select controls where possible. Map controls expand from 36 px on desktop to 44 px on phone; retain those touch targets in production.

## Page layouts and responsive rules

**Home:** hero / four server metrics / main activity column and 300 px side rail. Main column: two live game cards, recent-game table. Rail: online players, server explanation. At <=820 px the rail moves below activity; at <=560 px cards stack and metrics become a 2×2 grid. Lead with live games rather than a large account creation form.

**Game:** breadcrumb and match heading / 2-column tactical view + scoreboard rail. Desktop rail 380 px (330 px on small desktop); map height 455 px. The left column holds live/replay toggle, floor selector, map, legend, scrubber and game metadata. The right holds team/FFA table and kill feed. At <=820 px use one column; on phone order map → controls → metadata → scoreboard → feed. Map 305 px tall and all markers fitted into bounds. An ended game replaces Live with “Final,” defaults to replay at the final tick, shows outcome and end reason, and calls the page a carnage report. `quit` / `lost` show “Ended early” / “Connection lost,” without inventing a winner. A deep link can open a replay time without disturbing other views.

**Players:** heading, search/presence filter, directory grid (3 desktop / 2 tablet / 1 phone), result count and cursor controls when more data exists. Sort by gamertag unless explicitly choosing a different order; never imply a global ranking. Search can use loaded accounts only if labelled as such; obtain complete pagination or implement a backed search before claiming server-wide results. The mockup searches nine local samples.

**Player:** identity hero, four metrics, recent games and by-variant table; rail contains account metadata and activity. On phone the monogram shrinks, the full identity wraps, metric cells become 2×2, the rail comes last. The current-game link stays available in the dossier when the desktop hero aside disappears. Avoid badges implying official skill or achievements the API does not supply.

**Event log:** filters above a 2-column log / inspector layout, with stream status beneath the table. Desktop inspector 350 px; phone inspector below the log, linked by the selected position. Show raw event type, global position, timestamp, stream, author/key metadata and signature outcome. Long raw strings wrap in detail and remain copyable. The API returns oldest first: the mockup's “Newest first” is a client presentation choice for the loaded window, not a reversed server cursor. Deduplicate/order by global position and resume from the last processed position according to the endpoint's semantics. Show “Caught up” only after the stream's `caught_up` message. Offer pause-follow without stopping ingestion, so selected rows don't jump. Only #1042 has a detail preview in this static study; production binds every row. Raw payload must be rendered as text, never HTML.

**Global responsive:** 1280 px maximum content width; <=1100 px reduce gutters and side rails; <=820 px major columns stack; <=560 px navigation compresses, filters wrap, hero copy wraps, cards use one column. Tested deliverable widths are 1440 and 390 px; layout also targets 320 px without page-wide horizontal scrolling. Long data tables scroll in a labelled focusable region instead of hiding numeric columns. Keep table headers available; a production sticky header must not cover focus. Scroll is only inside the table. Inputs stay usable at zoom. Do not hide server identity or error explanations to make a phone screenshot shorter.

## Empty, loading, error and connection states

| State | Presentation and copy | Action / behaviour |
| --- | --- | --- |
| No live games | Static dark grid, “The lobby is quiet.” / “No live games on this server right now.” | Recent games and players remain available; no spinning radar |
| No account matches | “No matching callsigns.” / “Try another gamertag or clear the presence filter.” | Clear filters; keep query visible |
| No history | “Your first chapter is still ahead.” | Explain this server has no completed games for the account |
| Map 404 | “Map outline not available.” / “Positions will appear when this server receives an outline.” | Scoreboard, roster, kills and metadata continue working |
| Replay missing | “No recorded frames for this interval.” | Show gap in timeline; keep event/score data; do not animate through it |
| Initial loading | Stable panel/row placeholders, low-contrast static blocks, “Loading games…” | `aria-busy=true` on region; no fake values; no endless shimmer |
| Filter loading | Retain previous results with a visible updating label | Cancel stale request; commit only matching response |
| Request error | Coral left edge, “Couldn’t load games.” + safe server message | Retry button scoped to the panel; no cryptic code as sole copy |
| Invalid URL / discovery | Inline field error, “No h2 live service found at this address.” | Keep typed URL and existing connection; allow retry/edit |
| WebSocket reconnect | Amber banner, “Connection interrupted. Showing the last received update at 20:14:03 UTC.” | Freeze map; remove live claim; reconnect/resume from last position; keep server switcher |
| Replay download interrupted | Keep existing buffered frames, mark unbuffered range | Retry missing range; do not reset scrubber to zero |
| No longer existing record | “This game isn’t available on this server.” | Back to overview / check selected server |
| Invalid event signature | Coral `! Invalid signature`, reason in inspector | Retain inspectable raw record; never silently turn it green |
| Verification unavailable | Amber `◇ Unverified · key unavailable` | Retry key lookup as appropriate; don't label invalid |

Connection banners are separate from content so a disconnect does not erase useful records. Never use toast-only errors for a persistent failure. State copy is short, direct and a little warm; avoid fake militaristic jargon for basic errors.

## Applying this to SolidJS

1. Import tokens and add the provided `@font-face` definitions with URLs adjusted for the project's asset pipeline. Keep font licences in the distributed source. Scope panel/table styles in actual components.
2. Reuse a shared application shell and server-context store. Suggested components: `ServerSwitcher`, `PageNav`, `Panel`, `GameCard`, `MapFrame`, `ReplayControls`, `Scoreboard`, `KillFeed`, `PlayerCard`, `PresenceBadge`, `SignatureStatus`, `EventInspector`.
3. Key game markers by slot and records by global event position. Keep high-frequency map updates isolated from slower score/account rendering. Do not rerender the whole app 15 times per second. Scope sockets and fetches to the selected base URL; clean them up on navigation/server change.
4. Render the real map with SVG initially (appropriate for the small player count); memoize static polygons and update only marker transforms. SVG is not permission to ship proprietary geometry. Use the demo SVG only in design previews/tests clearly marked as such.
5. Route placeholders in mockups all lead to one sample game/profile. Replace with actual game IDs and server-qualified account identities; don't carry sample counts, “all systems connected,” verification results, or service-record aggregates into production as defaults.
6. Treat server and player strings as untrusted text. Use standard escaped text bindings and safe URL construction. Signature inspection must use the cryptographic implementation, not a `jws`-present heuristic. All auth/security rules remain owned by `docs/live-security.md`.
7. Static mockups are an acceptance reference for hierarchy, dimensions, colour and responsive behaviour. The engineer should preserve the app's working API semantics rather than importing this demonstration script.

## Deliverable inventory and review

- `DESIGN.md` — this specification.
- `tokens.css` — namespaced reusable tokens and reduced-motion overrides.
- `mockup.css` — shared static page styling and local font faces.
- `mockup.js` — limited, local demonstration interactions.
- `capture.sh` — repeatable `bunx playwright` screenshot capture.
- `home.html`, `game.html`, `players.html`, `player.html`, `events.html` — independent complete static pages.
- `map-schematic.svg`, `signal-lens.svg` — original art; the map is also inlined in HTML for easy component comparison.
- `fonts/` — five TTF files, three OFL licence files.
- Screenshots: `/Users/afr/.config/rush/keep/h2recomp-b5afe71a/bed6c8dc/design-shots/{home,game,players,player,events}-{desktop,phone}.png` (1440 / 390 px wide; full-page).

API basis: `docs/live-api.md`; discovery/server identity basis: `docs/live-security.md` §4. Those contracts take precedence if implementation details differ from this proposal.

### Completed browser review

Captured all five pages with `bunx playwright` / Chromium at 1440×1000 and 390×844 viewports, full-page, then visually inspected and refined the results. A separate browser pass checked all five pages at 1440, 390 and 320 px: no page-wide horizontal overflow, broken images, duplicate IDs or page JavaScript errors; all five font faces loaded. Wide data tables intentionally scroll inside their own labelled region.

Exercised player search and no-results state, event-type filtering, replay scrubbing and return to live, server URL preview and Escape dismissal, and reduced-motion token override. Fixed the first-pass nested-grid overflow, phone filter widths, clipped activity-chart labels, and map selection/slot consistency. The report is saved beside screenshots as `browser-review.json`; `final-contact-sheet.webp` is a quick overview of the final page set. This is a visual/browser verification, not a complete assistive-technology or WCAG conformance audit.
