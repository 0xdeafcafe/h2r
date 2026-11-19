// What kind of game a game was, in words: a custom game, matchmaking or
// Clankermatch (with its playlist), or campaign (solo, co-op on one console, or co-op online),
// with the campaign's difficulty. Pure, so pages and tests share it.

import type { GameState } from "./reducer";
import { titleCase } from "../lib/format";
import { gametypeKey } from "../lib/gameart";
import { variantType } from "./gameart";

export type GameKind = "custom" | "matchmaking" | "clankermatch" | "campaign";

type KindOf = Pick<GameState, "mode" | "kind">;

/** The game's kind: what it said, else inferred (campaign from its mode,
 *  anything else a custom game). A kind that contradicts the mode loses to
 *  the mode, so a campaign game never lands in multiplayer. */
export function gameKind(g: KindOf): GameKind {
  const campaign = g.mode === "campaign";
  if (campaign) return "campaign";
  return g.kind === "matchmaking" || g.kind === "clankermatch" ? g.kind : "custom";
}

const DIFFICULTIES = ["easy", "normal", "heroic", "legendary"];

/** A campaign difficulty for display, or null when unknown. */
export function difficultyName(d: string | null | undefined): string | null {
  if (!d) return null;
  const k = d.toLowerCase();
  return DIFFICULTIES.includes(k) ? titleCase(k) : titleCase(d);
}

/** A variant's name for display. The game's own names ("King of the Hill",
 *  "1 Flag CTF") keep their casing; a bare key ("team_slayer") is
 *  title-cased. */
export function variantName(v: string | null | undefined): string {
  const t = (v ?? "").trim();
  return /[A-Z ]/.test(t) ? t : titleCase(t);
}

/** A variant with its game type when its name doesn't say it: "Rockets
 *  (Slayer)", "2 on 1 (Juggernaut)", but "Team Slayer" and "Crazy King" as
 *  they are. `typeOf` names a variant's game type (the server's catalog by
 *  default, which lists the disc's built-in variants); a variant it
 *  doesn't know is shown alone. */
export function variantLabel(v: string | null | undefined, typeOf: (v: string) => string | null = variantType): string {
  const name = variantName(v);
  if (!name) return "";
  const type = typeOf(name);
  if (!type) return name;
  const says = gametypeKey(name);
  return says && says === gametypeKey(type) ? name : `${name} (${type})`;
}

type LabelOf = Pick<GameState, "mode" | "kind" | "playlist" | "online" | "players">;

/** "Campaign", "Campaign Co-op" (more than one player on one machine) or
 *  "Campaign Online" (partners on other machines). */
export function campaignLabel(g: Pick<GameState, "online" | "players">): string {
  if (g.online) return "Campaign Online";
  return Object.keys(g.players).length > 1 ? "Campaign Co-op" : "Campaign";
}

/** The type of game: "Custom Game", "Matchmaking: <playlist>",
 *  "Clankermatch: <playlist>" (against bots), or a campaign label. */
export function gameTypeLabel(g: LabelOf): string {
  switch (gameKind(g)) {
    case "campaign": return campaignLabel(g);
    case "matchmaking": return g.playlist ? `Matchmaking: ${g.playlist}` : "Matchmaking";
    case "clankermatch": return g.playlist ? `Clankermatch: ${g.playlist}` : "Clankermatch";
    default: return "Custom Game";
  }
}

/** One line for a game: the variant and its type for multiplayer
 *  ("Slayer · Custom Game"), the campaign label and difficulty for
 *  campaign ("Campaign Co-op · Heroic"). */
export function gameTypeLine(g: LabelOf & Pick<GameState, "variant" | "difficulty">): string {
  if (gameKind(g) === "campaign") return [campaignLabel(g), difficultyName(g.difficulty)].filter(Boolean).join(" · ");
  return [g.variant ? variantLabel(g.variant) : null, gameTypeLabel(g)].filter(Boolean).join(" · ");
}
