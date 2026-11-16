// The devices an account is signed in on: its keys
// (GET /v1/accounts/{gamertag}/keys), each named by its kid and dated by
// the log event that added it (GET /v1/events?from=<position - 1>&limit=1).

import type { AccountKeyEntry } from "../api/types";

export interface Device {
  kid: string;
  /** The log position the key is valid from. */
  from: number | null;
  /** When the key was added, if the log said. */
  since: string | null;
  current: boolean;
}

/** A kid short enough to read: its first eight characters. */
export const shortKid = (kid: string) => kid.slice(0, 8);

export function parseKeys(body: unknown): AccountKeyEntry[] {
  const keys = (body as { keys?: unknown } | null)?.keys;
  if (!Array.isArray(keys)) return [];
  return keys.filter((k): k is AccountKeyEntry => !!k && typeof k === "object" && typeof (k as { kid?: unknown }).kid === "string");
}

/** The account's keys, newest first, each with the time it was added. */
export async function fetchDevices(api: string, gamertag: string, fetchFn: typeof fetch = fetch): Promise<Device[]> {
  const r = await fetchFn(`${api}/accounts/${encodeURIComponent(gamertag)}/keys`, { headers: { accept: "application/json" } });
  if (!r.ok) throw new Error(`the server answered HTTP ${r.status}`);
  const keys = parseKeys(await r.json());
  const out = await Promise.all(keys.map(async (k): Promise<Device> => {
    const from = typeof k.valid_from === "number" ? k.valid_from : null;
    let since: string | null = null;
    if (from !== null && from > 0) {
      try {
        const e = await fetchFn(`${api}/events?from=${from - 1}&limit=1`, { headers: { accept: "application/json" } });
        if (e.ok) {
          const item = ((await e.json()) as { items?: Array<{ position?: number; at?: string }> }).items?.[0];
          if (item?.position === from && typeof item.at === "string") since = item.at;
        }
      } catch { /* the date is a nicety */ }
    }
    return { kid: k.kid, from, since, current: k.valid_to === null };
  }));
  return out.sort((a, b) => Number(b.current) - Number(a.current) || (b.from ?? 0) - (a.from ?? 0));
}
