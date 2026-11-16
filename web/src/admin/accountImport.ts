// Signing the browser in as an account the CLI holds: `h2 live account
// export <gamertag> --file f.json` writes the entry this accepts (or prints
// it for pasting). The key is checked against the server (the same check
// `h2 live account import` makes in the other direction) before it is kept
// in this browser, where anyone with the profile can read it. The
// dashboard's import screen (/dash/server/import) is the page for it.

import type { LocalAccount } from "../state/account";
import { importSeed } from "../state/account";
import { parseImportEntry } from "./validate";

interface KeyEntry {
  kid: string;
  jwk: { x?: string };
  valid_to: number | null;
}

export interface ImportTarget {
  /** The server's API root (`…/v1`). */
  api: string;
  /** Its base URL and id, kept with the account. */
  base: string;
  id: string;
  /** Keeps the account in this browser and signs in as it. */
  add(account: LocalAccount): Promise<void>;
  fetch?: typeof fetch;
}

export type ImportResult = { ok: true; gamertags: string[] } | { ok: false; error: string };

/** Checks each pasted account against the server and keeps the good ones;
 *  stops at the first that fails. */
export async function importAccounts(text: string, to: ImportTarget): Promise<ImportResult> {
  const parsed = parseImportEntry(text);
  if (!parsed.ok) return { ok: false, error: parsed.error };
  const f = to.fetch ?? fetch;
  const done: string[] = [];
  try {
    for (const account of parsed.accounts) {
      if (account.server_id && account.server_id !== to.id) {
        return { ok: false, error: `${account.gamertag} is an account of server ${account.server_id}, not this one` };
      }
      const r = await f(`${to.api}/accounts/by-xuid/${encodeURIComponent(account.xuid)}/keys`, { headers: { accept: "application/json" } });
      if (!r.ok) return { ok: false, error: `this server doesn't know ${account.gamertag}` };
      const keys = ((await r.json()) as { keys?: KeyEntry[] }).keys ?? [];
      const key = await importSeed(account.key, account.x);
      const current = keys.some((k) => k.valid_to === null && k.kid === key.publicJwk.kid && k.jwk.x === account.x);
      if (!current) return { ok: false, error: `${account.gamertag}'s key is not current on this server` };
      await to.add({
        xuid: account.xuid,
        gamertag: account.gamertag,
        id: account.id,
        server: to.base,
        server_id: to.id,
        kid: key.publicJwk.kid,
        key: account.key,
        x: account.x,
      });
      done.push(account.gamertag);
    }
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
  return { ok: true, gamertags: done };
}
