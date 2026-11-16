// Which half of the stats a page shows, from its URL: multiplayer unless the
// path or `?tab=` asks for campaign. Pure, so routing is testable.

import type { GameMode } from "../state/reducer";

export type SpotlightTab = "live" | "recent" | "team" | "ffa" | "campaign";
const SPOTLIGHT: SpotlightTab[] = ["live", "recent", "team", "ffa", "campaign"];

/** The online page's spotlight tab: the one asked for, else live games
 *  when there are any, else recent (multiplayer) games. */
export function spotlightTab(asked: string | null | undefined, anyLive: boolean): SpotlightTab {
  return SPOTLIGHT.includes(asked as SpotlightTab) ? (asked as SpotlightTab) : anyLive ? "live" : "recent";
}

export type HistoryTab = "all" | "team" | "ffa" | "campaign";
const HISTORY: HistoryTab[] = ["all", "team", "ffa", "campaign"];

/** A player's game history tab; multiplayer ("all") by default. */
export function historyTab(asked: string | null | undefined): HistoryTab {
  return HISTORY.includes(asked as HistoryTab) ? (asked as HistoryTab) : "all";
}

export const historyMode = (t: HistoryTab): GameMode => (t === "campaign" ? "campaign" : "multiplayer");

/** A player's service record for one half of the stats. */
export function recordHref(gamertag: string, mode: GameMode): string {
  const base = `/stats/players/${encodeURIComponent(gamertag)}`;
  return mode === "campaign" ? `${base}/campaign` : base;
}
