// The `/signin#once=…` link `h2 play --live` prints: one use, this machine
// only (docs/live-admin.md §4). The token rides in the fragment so it never
// reaches a server log or a Referer; the page takes it out of the address
// bar first, then trades it at `POST /v1/local/signin` for the browser's
// account entry — the same entry Admin > Import account accepts, checked
// the same way (its key must be current on this server) before it is kept.

import { parseImportEntry } from "../admin/validate";
import { importSeed, type LocalAccount } from "../state/account";

/** The token in a `#once=…` fragment, or null. */
export function takeOnce(hash: string): string | null {
  const params = new URLSearchParams(hash.replace(/^#/, ""));
  const once = params.get("once");
  return once && /^[A-Za-z0-9_-]{16,128}$/.test(once) ? once : null;
}

/** Takes the token out of the address bar (and history) before anything
 *  else can see it; returns it. */
export function clearOnce(loc: { hash: string; pathname: string; search: string }, history: Pick<History, "replaceState" | "state">): string | null {
  const once = takeOnce(loc.hash);
  if (loc.hash) history.replaceState(history.state, "", loc.pathname + loc.search);
  return once;
}

export type RedeemResult =
  | { ok: true; account: LocalAccount; admin: boolean }
  | { ok: false; kind: "gone" | "failed"; error: string };

export const GONE =
  "This sign-in link has been used or has expired (each link works once, for ten minutes, on this machine only). Run h2 play --live again for a new link.";

interface KeyEntry {
  kid: string;
  jwk: { x?: string };
  valid_to: number | null;
}

/** Redeems `once` against the server at `base` (`api` is its API root,
 *  `serverId` its well-known id). */
export async function redeem(
  once: string,
  server: { base: string; api: string; serverId: string },
  fetchFn: typeof fetch = fetch,
): Promise<RedeemResult> {
  const failed = (error: string): RedeemResult => ({ ok: false, kind: "failed", error });
  let r: Response;
  try {
    r = await fetchFn(`${server.api}/local/signin`, {
      method: "POST",
      headers: { "content-type": "application/json", accept: "application/json" },
      body: JSON.stringify({ once }),
      cache: "no-store",
      credentials: "omit",
    });
  } catch (e) {
    return failed(`the live server did not answer (${e instanceof Error ? e.message : String(e)})`);
  }
  if (r.status === 404) return { ok: false, kind: "gone", error: GONE };
  if (r.status === 429) return failed("too many tries; wait a few seconds and reload");
  if (!r.ok) return failed(`the server refused the link (HTTP ${r.status})`);
  const parsed = parseImportEntry(await r.text());
  if (!parsed.ok) return failed(`the server's answer is not an account: ${parsed.error}`);
  const entry = parsed.accounts[0]!;
  if (entry.server_id && server.serverId && entry.server_id !== server.serverId) {
    return failed(`${entry.gamertag} is an account of server ${entry.server_id}, not this one`);
  }
  // The same check the Admin area's import makes: the key is current here.
  const keys = await fetchFn(`${server.api}/accounts/by-xuid/${encodeURIComponent(entry.xuid)}/keys`, {
    headers: { accept: "application/json" },
  });
  if (!keys.ok) return failed(`the server does not know xuid ${entry.xuid}`);
  const list = ((await keys.json()) as { keys?: KeyEntry[] }).keys ?? [];
  const key = await importSeed(entry.key, entry.x);
  if (!list.some((k) => k.valid_to === null && k.kid === key.publicJwk.kid && k.jwk.x === entry.x)) {
    return failed(`${entry.gamertag}'s key is not current on this server`);
  }
  let admin = false;
  try {
    const a = await fetchFn(`${server.api}/accounts/${encodeURIComponent(entry.gamertag)}`, {
      headers: { accept: "application/json" },
    });
    if (a.ok) admin = ((await a.json()) as { admin?: boolean }).admin === true;
  } catch { /* not knowing only changes where we go next */ }
  return {
    ok: true,
    admin,
    account: {
      xuid: entry.xuid,
      gamertag: entry.gamertag,
      id: entry.id,
      server: server.base,
      server_id: server.serverId || entry.server_id,
      kid: key.publicJwk.kid,
      key: entry.key,
      x: entry.x,
    },
  };
}
