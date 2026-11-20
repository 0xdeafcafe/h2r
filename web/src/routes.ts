// Where the sites live. The stats site is Halo 2's, under /games/ so
// another game's (Halo 1's) can sit beside it. The servers know the same
// root (crates/live/live-server/src/page_meta.rs, STATS_ROOT) for link previews
// and the old /stats redirect.

/** Each game's stats site sits under this. Alone, it goes to Halo 2's. */
export const GAMES_ROOT = "/games";

/** The stats site (STATS_NAME): Halo 2's games, players and leaderboards. */
export const STATS_ROOT = `${GAMES_ROOT}/halo2`;

/** Where the stats site lived first; its links redirect to STATS_ROOT. */
export const OLD_STATS_ROOT = "/stats";

/** A page of the stats site: `statsPath("/players/x")` is
 *  `/games/halo2/players/x`; no argument is its home. */
export const statsPath = (page = ""): string => `${STATS_ROOT}${page}`;

/** The same page under STATS_ROOT for a path under OLD_STATS_ROOT, or
 *  null for any other path. Query and hash are the caller's to keep. */
export function movedStatsPath(path: string): string | null {
  if (path !== OLD_STATS_ROOT && !path.startsWith(`${OLD_STATS_ROOT}/`)) return null;
  const rest = path.slice(OLD_STATS_ROOT.length);
  return statsPath(rest === "/" ? "" : rest);
}

export const GAME_VIEWER_ID = "game-viewer";
const GAME_VIEWER_HASH = `#${GAME_VIEWER_ID}`;

export function gameHref(game: string, options?: { player?: string }): string {
  const path = statsPath(`/games/${encodeURIComponent(game)}`);
  return options?.player ? `${path}?player=${encodeURIComponent(options.player)}` : path;
}

/** Whole films open in Game Details. Shared clips keep their distinct URL. */
export function theaterHref(game: string, clip?: string): string {
  return clip === undefined
    ? gameHref(game) + GAME_VIEWER_HASH
    : `/theater/${encodeURIComponent(game)}/clip/${encodeURIComponent(clip)}`;
}

export function movedTheaterPath(path: string, search = ""): string | null {
  const match = /^\/theater\/([^/]+)\/?$/.exec(path);
  return match ? statsPath(`/games/${match[1]}`) + search + GAME_VIEWER_HASH : null;
}

export function clipShareUrl(origin: string, game: string, clip: string): string {
  return new URL(theaterHref(game, clip), origin).toString();
}
