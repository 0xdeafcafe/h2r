// Which half of the stats a page shows, from its URL: multiplayer unless the
// path or `?tab=` asks for campaign. Pure, so routing is testable.

import type { GameMode } from "../state/reducer";
import { STATS_NAME } from "../brand";

export type SpotlightTab = "live" | "recent" | "team" | "ffa" | "campaign";
const SPOTLIGHT: SpotlightTab[] = ["live", "recent", "team", "ffa", "campaign"];

/** The online page's spotlight tab: the one asked for, else live games
 *  when there are any, else recent (multiplayer) games. */
export function spotlightTab(asked: string | null | undefined, anyLive: boolean): SpotlightTab {
  return SPOTLIGHT.includes(asked as SpotlightTab) ? (asked as SpotlightTab) : anyLive ? "live" : "recent";
}

export type HistoryTab = "all" | "team" | "ffa" | "campaign" | "coop";
const HISTORY: HistoryTab[] = ["all", "team", "ffa", "campaign", "coop"];

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

/** A clan's page. Clans are the protocol's teams (Xbox Live Teams); the
 *  site says clan, and /stats/teams/:id redirects here. */
export const clanHref = (id: string) => `/stats/clans/${encodeURIComponent(id)}`;

/** The stats site's home: its name (STATS_NAME) in the nav, "Home" in the crumbs. */
export const STATS_HOME = "/stats";
export const HOME_CRUMB: [string, string] = ["Home", STATS_HOME];

/** The way back to the dashboard: kept out of the nav tabs, in the header's corner. */
export const DASHBOARD_EXIT = { label: "‹ Dashboard", href: "/", title: "Leave the stats site for the dashboard" } as const;

export interface NavItem { label: string; href: string; end?: boolean }

/** The nav strip's tabs, home first. The dashboard is not one of them. */
export function statsNav(me: { gamertag: string } | null, admin: boolean): NavItem[] {
  return [
    { label: STATS_NAME, href: STATS_HOME, end: true },
    { label: "My Stats", href: me ? `/stats/players/${encodeURIComponent(me.gamertag)}` : "/dash/signup" },
    { label: "Games", href: "/stats/games" },
    { label: "Players", href: "/stats/players", end: true },
    { label: "Clans", href: "/stats/clans" },
    { label: "Event Log", href: "/stats/events" },
    ...(admin ? [{ label: "Halo Admin", href: "/stats/admin", end: true }] : []),
    { label: "Server", href: "/dash/server" },
  ];
}
