// Which live server the app talks to. Defaults to the page's own origin;
// can be switched (remembered in localStorage) or deep-linked with ?server=.

export interface WellKnown {
  id: string;
  api?: string;
  keys?: Array<{ kty: string; crv: string; x: string; kid?: string }>;
  software?: string;
  federation?: boolean;
}

export interface ServerTarget {
  /** Base URL without trailing slash, e.g. "http://127.0.0.1:8074". */
  base: string;
  /** API root ("<base>/v1" unless the server's well-known says otherwise). */
  api: string;
}

const KEY_CURRENT = "h2live.server";
const KEY_RECENT = "h2live.recentServers";

export function normalizeBase(input: string, fallbackProtocol = "http:"): string {
  let s = input.trim();
  if (!s) throw new Error("empty server address");
  if (!/^[a-z][a-z0-9+.-]*:\/\//i.test(s)) {
    const local = /^(localhost|127\.|\[::1\]|0\.0\.0\.0)/i.test(s);
    s = (local ? "http:" : fallbackProtocol === "https:" ? "https:" : "http:") + "//" + s;
  }
  const u = new URL(s);
  if (u.protocol !== "http:" && u.protocol !== "https:") throw new Error("server must be http or https");
  let path = u.pathname.replace(/\/+$/, "");
  // Accept a pasted API or well-known URL.
  path = path.replace(/\/(v1|\.well-known\/h2live\.json)$/, "");
  return u.origin + path;
}

export function wellKnownUrl(base: string): string {
  return base + "/.well-known/h2live.json";
}

/** The API root: the well-known `api`, which may be relative, resolved
 *  against the well-known document's own URL. */
export function targetFor(base: string, wk?: WellKnown | null): ServerTarget {
  let api = base + "/v1";
  if (wk?.api) {
    try {
      api = new URL(wk.api, wellKnownUrl(base)).toString().replace(/\/+$/, "");
    } catch { /* keep default */ }
  }
  return { base, api };
}

export function wsUrl(httpUrl: string): string {
  const u = new URL(httpUrl, typeof location !== "undefined" ? location.href : undefined);
  u.protocol = u.protocol === "https:" ? "wss:" : "ws:";
  return u.toString();
}

export function defaultBase(): string {
  return typeof location !== "undefined" ? location.origin : "http://127.0.0.1:8074";
}

function storage(): Storage | null {
  try {
    return typeof localStorage !== "undefined" ? localStorage : null;
  } catch {
    return null;
  }
}

/** The server to use at boot: ?server= wins, then the remembered one. */
export function initialBase(): string {
  if (typeof location !== "undefined") {
    const q = new URLSearchParams(location.search).get("server");
    if (q) {
      try {
        const b = normalizeBase(q, location.protocol);
        remember(b);
        return b;
      } catch { /* ignore a bad deep link */ }
    }
  }
  return storage()?.getItem(KEY_CURRENT) ?? defaultBase();
}

export interface RecentServer { base: string; id?: string; lastUsed: number }

export function recentServers(): RecentServer[] {
  try {
    const raw = storage()?.getItem(KEY_RECENT);
    const list = raw ? (JSON.parse(raw) as RecentServer[]) : [];
    return Array.isArray(list) ? list.filter((r) => r && typeof r.base === "string") : [];
  } catch {
    return [];
  }
}

export function remember(base: string, id?: string): void {
  const s = storage();
  if (!s) return;
  const list = recentServers().filter((r) => r.base !== base);
  list.unshift({ base, id, lastUsed: Date.now() });
  s.setItem(KEY_RECENT, JSON.stringify(list.slice(0, 8)));
  if (base === defaultBase()) s.removeItem(KEY_CURRENT);
  else s.setItem(KEY_CURRENT, base);
}

export function forget(base: string): void {
  const s = storage();
  if (!s) return;
  s.setItem(KEY_RECENT, JSON.stringify(recentServers().filter((r) => r.base !== base)));
}

export async function fetchWellKnown(base: string, signal?: AbortSignal): Promise<WellKnown> {
  const r = await fetch(wellKnownUrl(base), { signal, headers: { accept: "application/json" } });
  if (!r.ok) throw new Error(`no live server there (HTTP ${r.status})`);
  const wk = (await r.json()) as WellKnown;
  if (!wk || typeof wk.id !== "string") throw new Error("that doesn't look like a live server");
  return wk;
}
