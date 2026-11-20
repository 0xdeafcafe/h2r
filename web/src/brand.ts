// The product's names and colours, in one place. index.html gets the names
// at build time (vite.config.ts), and the server reads them back out of it
// for link previews (crates/live/live-server/src/page_meta.rs), so a rename is
// a one-line change here. Routes never change with a name.

/** The service's name: the link-preview card's mark, and what the CLI and
 *  server call themselves. Neither site is named this. */
export const SITE_NAME = "h2 live";

/** The dashboard's name (/dash): the app the manifest installs, the page
 *  titles and the header. */
export const DASH_NAME = "Live Dashboard";

/** The stats site's name (/games/halo2): where the dashboard sends you for
 *  games, service records and leaderboards. The options considered are in
 *  docs/live/web-app.md, "Names, icons and link previews". */
export const STATS_NAME = "h2 Uplink";

/** The browser's colour on each side, and the web app manifest's: the
 *  dashboard's dark green and the stats site's navy (styles/theme.css).
 *  page_meta.rs carries the same two. */
export const THEME = { dash: "#0c4406", stats: "#071a35" } as const;

/** The manifest's splash background: the dashboard's black. */
export const BACKGROUND = "#010a01";

/** index.html's placeholders and what they become. */
export const HTML_NAMES: Record<string, string> = {
  "%H2_DASH_NAME%": DASH_NAME,
  "%H2_STATS_NAME%": STATS_NAME,
};

/** `html` with every placeholder of [`HTML_NAMES`] filled in, escaped
 *  for an attribute or text. */
export function fillNames(html: string): string {
  const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  return Object.entries(HTML_NAMES).reduce((h, [k, v]) => h.split(k).join(esc(v)), html);
}
