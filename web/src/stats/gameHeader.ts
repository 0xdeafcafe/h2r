// The pre-rendered game header is mirrored by live-server's page_header.rs.
// Fixtures hold both renderers to the same markup and client adoption keeps
// the existing DOM while its model still matches.

import type { GameState } from "../state/reducer";
import { emblemArt, packEmblem } from "../lib/art";
import { emblemPath } from "../lib/emblem";
import { titleCase } from "../lib/format";

// ---- the model ----

/** One player in the header: their name, their emblem, their standing. */
export interface HeaderPlayer {
  name: string;
  /** What the emblem is picked from: the XUID when known, else the name. */
  seed: string;
  /** Fallback from the art pack. */
  emblem: number;
  /** The composed game-art emblem. Older page metadata omits it. */
  emblemSrc?: string;
  /** This player's colour, `#rrggbb`. */
  colour: string;
  /** A bot (a Clanker): marked beside the name. Left out when not. */
  bot?: true;
  /** Their score. Left out when the game has none (a campaign). */
  score?: number;
}

/** One team in the header: its colour, its score, its players. */
export interface HeaderTeam {
  team: number;
  name: string;
  colour: string;
  score: number;
  players: HeaderPlayer[];
}

/** What the header shows, as the servers ship it into the page (and the
 *  client re-derives): plain JSON, so both sides render the same bytes. */
export interface GameHeaderModel {
  game: string;
  live: boolean;
  /** The variant's name ("Slayer"), or the map for a campaign. */
  title: string;
  /** The map's display name. */
  map: string;
  /** `LIVE` or `FINAL`. */
  status: string;
  /** How it ended ("Red Team won"); null while it is live. */
  result: string | null;
  /** The kind of game ("Custom Game, Team Game", "Campaign Co-op · Heroic"). */
  type: string;
  /** Team play: the teams. */
  teams?: HeaderTeam[];
  /** Free-for-all and campaign: the players, in standing order. */
  players?: HeaderPlayer[];
  /** A campaign game: the title is the mission, not "on" a map. */
  campaign?: true;
}

// ---- shared constants (mirrored in page_header.rs) ----

/** Team colours, by team index (map/palette.ts `FALLBACK.teams`, which the
 *  page's own CSS never overrides — `--map-team-<i>` is never defined). */
const TEAM_COLOURS = ["#ff6f61", "#4aa3ff", "#3cc48f", "#ffb627", "#a07cff", "#ff8fc7", "#5ad1e0", "#9b8a7a"];
/** Free-for-all colours, by player index (map/palette.ts `FALLBACK.ffa`). */
const FFA_COLOURS = [
  "#ff6f61", "#4aa3ff", "#3cc48f", "#ffb627", "#a07cff", "#ff8fc7", "#5ad1e0", "#c98b5c",
  "#e85d9b", "#6b8cff", "#8bd450", "#ff9152", "#7a6cff", "#40c4aa", "#d4a24a", "#b06cd9",
];

const teamColour = (team: number) => TEAM_COLOURS[((team % 8) + 8) % 8]!;
const ffaColour = (index: number) => FFA_COLOURS[index % 16]!;
export const teamName = (team: number): string =>
  ["Red", "Blue", "Green", "Gold", "Purple", "Pink", "Teal", "Brown"][team] ?? `Team ${team + 1}`;

// ---- deriving the model from a game ----

/** A variant's name for display (state/gametype.ts `variantName`). */
function variantName(v: string | null | undefined): string {
  const t = (v ?? "").trim();
  return /[A-Z ]/.test(t) ? t : titleCase(t);
}

/** A campaign difficulty for display (state/gametype.ts `difficultyName`). */
function difficultyName(d: string | null | undefined): string | null {
  return d ? titleCase(d) : null;
}

/** The kind of game for the header's type line, as the servers derive it
 *  too (playlist() in stats/Side.tsx): "Matchmaking: <playlist>",
 *  "Clankermatch: <playlist>", "Custom Game, Team Game", or the campaign
 *  label with its difficulty. */
function typeLine(g: GameState): string {
  if (g.mode === "campaign") {
    const label = g.online ? "Campaign Online" : Object.keys(g.players).length > 1 ? "Campaign Co-op" : "Campaign";
    return [label, difficultyName(g.difficulty)].filter(Boolean).join(" · ");
  }
  const match = g.kind === "matchmaking" || g.kind === "clankermatch";
  const label = match
    ? g.playlist ? `${g.kind === "matchmaking" ? "Matchmaking" : "Clankermatch"}: ${g.playlist}` : (g.kind === "matchmaking" ? "Matchmaking" : "Clankermatch")
    : `Custom Game, ${g.team_play ? "Team Game" : "Free for All"}`;
  return label;
}

/** How a finished game ended (resultWords in stats/Side.tsx, which the
 *  servers' link previews already mirror). */
function resultWords(g: GameState): string | null {
  if (g.status !== "ended") return null;
  const w = g.winner;
  if (!w) return g.end_reason === "quit" ? "Ended early" : g.end_reason === "lost" ? "Unfinished" : "No winner";
  return "team" in w ? `${teamName(w.team)} Team won` : `${g.players[w.player]?.name ?? "Someone"} placed 1st`;
}

const playerSeed = (p: GameState["players"][number]): string => p.xuid ?? p.name;

function headerPlayer(g: GameState, p: GameState["players"][number], withScores: boolean, base: string): HeaderPlayer {
  const score = g.scores[p.index]?.score;
  return {
    name: p.name,
    seed: playerSeed(p),
    emblem: packEmblem(playerSeed(p)),
    emblemSrc: base + emblemPath(playerSeed(p), p.appearance),
    colour: g.team_play ? teamColour(p.team) : ffaColour(p.index),
    ...(p.bot === true ? { bot: true as const } : {}),
    ...(withScores && score !== undefined ? { score } : {}),
  };
}

/** The header's model for a game, as the servers build it from the read
 *  model (page_header.rs `model_for`): the same fields, in the same order.
 *  Players stand by score then index; teams by score then team. */
export function gameHeaderModel(g: GameState, base = ""): GameHeaderModel {
  const campaign = g.mode === "campaign";
  const map = titleCase(g.map.split(/[\\/]/).pop()!.replace(/\.map$/i, "").trim());
  const live = g.status === "live";
  // The players in standing order (score, then index — the order the
  // servers' summaries use), each still knowing its team for the fold.
  const rows = Object.values(g.players)
    .slice()
    .sort((a, b) => (g.scores[b.index]?.score ?? 0) - (g.scores[a.index]?.score ?? 0) || a.index - b.index)
    .map((p) => ({ team: p.team, player: headerPlayer(g, p, !campaign, base) }));
  if (campaign || !g.team_play) {
    return {
      game: g.game_id, live,
      title: campaign ? map : variantName(g.variant) || titleCase(g.mode),
      map, status: live ? "LIVE" : "FINAL", result: resultWords(g), type: typeLine(g),
      players: rows.map((r) => r.player),
      ...(campaign ? { campaign: true as const } : {}),
    };
  }
  const by = new Map<number, HeaderPlayer[]>();
  for (const r of rows) {
    const list = by.get(r.team) ?? [];
    list.push(r.player);
    by.set(r.team, list);
  }
  const teams: HeaderTeam[] = [...by.entries()]
    .map(([team, ps]) => ({
      team,
      name: teamName(team),
      colour: teamColour(team),
      score: g.teams[team] ?? ps.reduce((n, p) => n + (p.score ?? 0), 0),
      players: ps,
    }))
    .sort((a, b) => b.score - a.score || a.team - b.team);
  return {
    game: g.game_id, live,
    title: variantName(g.variant) || titleCase(g.mode),
    map, status: live ? "LIVE" : "FINAL", result: resultWords(g), type: typeLine(g),
    teams,
  };
}

// ---- rendering ----

/** HTML escaping, as the servers do it (page_meta.rs `escape`). */
export function escapeHtml(s: string): string {
  let out = "";
  for (const c of s) {
    out += c === "&" ? "&amp;" : c === "<" ? "&lt;" : c === ">" ? "&gt;"
      : c === "\"" ? "&quot;" : c === "'" ? "&#39;"
      : c.charCodeAt(0) < 0x20 ? " " : c;
  }
  return out;
}

/** Fixed dimensions reserve space while the image loads. */
function emblemImg(player: HeaderPlayer, size: number): string {
  const e = escapeHtml;
  const { src, srcset } = emblemArt(player.emblem, size);
  const image = player.emblemSrc
    ? `class="emblem emblem--game" src="${e(player.emblemSrc)}" data-emblem-fallback="${e(src)}"`
    : `class="emblem" src="${e(src)}" srcset="${e(srcset)}"`;
  return `<img ${image} width="${size}" height="${size}" alt="" decoding="async" draggable="false">`;
}

/** The header's markup. One line, no whitespace between tags, so the
 *  client's render and the servers' are the same bytes (the parity test
 *  holds them to that). */
export function gameHeaderHtml(m: GameHeaderModel): string {
  const e = escapeHtml;
  const title = `<b>${e(m.title)}</b>${m.campaign ? "" : ` on ${e(m.map)}`}`;
  const sub = `<i class="s-gamehead-flag${m.live ? " s-gamehead-flag--live" : ""}">${e(m.status)}</i>`
    + (m.result ? `<span>${e(m.result)}</span>` : "")
    + `<small>${e(m.type)}</small>`;
  const player = (p: HeaderPlayer) =>
    `<li class="s-gamehead-player" style="--row:${e(p.colour)}">${emblemImg(p, 16)}<span>${e(p.name)}</span>`
    + (p.bot ? `<i class="s-gamehead-bot">bot</i>` : "")
    + (p.score !== undefined ? `<b class="s-gamehead-pscore">${p.score}</b>` : "")
    + `</li>`;
  const side = m.teams
    ? `<ul class="s-gamehead-teams">${m.teams.map((t) =>
        `<li class="s-gamehead-team" style="--row:${e(t.colour)}"><header><span>${e(t.name)}</span><b>${t.score}</b></header>`
        + `<ul class="s-gamehead-players">${t.players.map(player).join("")}</ul></li>`).join("")}</ul>`
    : `<ul class="s-gamehead-players s-gamehead-players--solo">${(m.players ?? []).map(player).join("")}</ul>`;
  return `<div class="s-gamehead" data-game="${e(m.game)}">`
    + `<div class="s-gamehead-info"><p class="s-gamehead-title">${title}</p><p class="s-gamehead-sub">${sub}</p></div>`
    + `<div class="s-gamehead-side">${side}</div>`
    + `</div>`;
}

/** The model as the page carries it (a `<script type="application/json">`
 *  beside the header), byte-identical from either side: compact JSON, in
 *  the field order above, with `<` escaped so it cannot close the script. */
export function gameHeaderJson(m: GameHeaderModel): string {
  return JSON.stringify(m).replace(/</g, "\\u003c");
}

// ---- the page's node (client only) ----

/** The pre-rendered header's node, which the servers write into the page
 *  above the app (`index.html`'s `<!-- game-header -->` block) and the
 *  client keeps as it is. */
export const HEADER_ID = "game-header";
export const MODEL_ID = "game-header-model";

function node(): HTMLElement | null {
  return typeof document === "undefined" ? null : document.getElementById(HEADER_ID);
}

/** The header's node, made if the page shipped none (a dev server, or a
 *  client-side arrival at a game page): the same shape the servers write
 *  — the shell, the model's script, the column — above the app. */
function ensureNode(): HTMLElement | null {
  if (typeof document === "undefined") return null;
  let n = node();
  if (n) return n;
  n = document.createElement("div");
  n.id = HEADER_ID;
  n.className = "s-gamehead-shell";
  n.innerHTML = `<script type="application/json" id="${MODEL_ID}"></script><div class="s-gamehead-col"></div>`;
  document.body.insertBefore(n, document.getElementById("root"));
  return n;
}

/** The model the page shipped, as the client finds it beside the header. */
export function embeddedModel(): GameHeaderModel | null {
  const s = typeof document === "undefined" ? null : document.getElementById(MODEL_ID)?.textContent;
  if (!s) return null;
  try { return JSON.parse(s) as GameHeaderModel; } catch { return null; }
}

/** Shows `m` in the page's header node: the pre-rendered markup is kept as
 *  it is when it already is this model (a page load — nothing repaints, so
 *  no flicker), and only rewritten when the game has actually moved on (a
 *  navigation, or a live game's score). `null` hides it; `undefined`
 *  leaves it alone (the game is still loading, and whatever the page
 *  shipped stays up). Returns what it did, for the tests. */
export function showGameHeader(m: GameHeaderModel | null | undefined): "hid" | "kept" | "wrote" {
  if (m === undefined) return "kept";
  const found = node();
  if (!found && m === null) return "hid";
  const n = found ?? ensureNode();
  if (!n) return "hid";
  if (m === null) {
    n.hidden = true;
    return "hid";
  }
  n.hidden = false;
  const json = gameHeaderJson(m);
  if (n.dataset.game === m.game && embeddedJson(n) === json) return "kept";
  n.dataset.game = m.game;
  const script = n.querySelector(`#${MODEL_ID}`);
  const col = n.querySelector(".s-gamehead-col");
  if (!script || !col) return "wrote";
  script.textContent = json;
  col.innerHTML = gameHeaderHtml(m);
  return "wrote";
}

/** The model beside the node itself, as a string, adoption's comparison. */
function embeddedJson(n: HTMLElement): string | null {
  return n.querySelector(`#${MODEL_ID}`)?.textContent ?? null;
}
