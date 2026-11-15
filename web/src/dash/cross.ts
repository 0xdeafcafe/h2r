// Which side of the site a path is on. The dashboard and the stats site
// look nothing alike, so going from one to the other plays a short veil
// (see Veil.tsx); moving about within a side does not.

export type Side = "dash" | "stats";

/** Milliseconds the veil takes to cover the old side, then to clear off the new one. */
export const CROSS_IN_MS = 200;
export const CROSS_OUT_MS = 320;

export function sideOf(path: string): Side {
  const p = path.replace(/[?#].*$/, "").replace(/\/+$/, "");
  // "/players" is an old link that redirects into the dashboard.
  return p === "" || p === "/dash" || p.startsWith("/dash/") || p === "/players" ? "dash" : "stats";
}

/** Whether following `href` from `from` changes sides. */
export const crosses = (from: string, href: string) => sideOf(from) !== sideOf(href);
