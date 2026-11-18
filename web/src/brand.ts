// The product's names and colours, in one place. index.html gets the names
// at build time (vite.config.ts), and the server reads them back out of it
// for link previews (crates/live-server/src/page_meta.rs), so a rename is
// a one-line change here. Routes never change with a name.

/** The service, and the dashboard's name. */
export const SITE_NAME = "h2 live";

/** The stats site's name (/stats): where the dashboard sends you for
 *  games, service records and leaderboards. The options considered are in
 *  web/README.md, "Names, icons and link previews". */
export const STATS_NAME = "h2 Uplink";

/** The browser's colour on each side, and the web app manifest's: the
 *  dashboard's dark green and the stats site's navy (styles/theme.css).
 *  page_meta.rs carries the same two. */
export const THEME = { dash: "#0c4406", stats: "#071a35" } as const;

/** The manifest's splash background: the dashboard's black. */
export const BACKGROUND = "#010a01";

/** index.html's placeholders and what they become. */
export const HTML_NAMES: Record<string, string> = {
  "%H2_SITE_NAME%": SITE_NAME,
  "%H2_STATS_NAME%": STATS_NAME,
};

/** `html` with every placeholder of [`HTML_NAMES`] filled in, escaped
 *  for an attribute or text. */
export function fillNames(html: string): string {
  const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  return Object.entries(HTML_NAMES).reduce((h, [k, v]) => h.split(k).join(esc(v)), html);
}
