// Whether the signed-in browser account is an admin on the current server.
// The account read API carries `admin: true` for admins (docs/live/live-admin.md);
// this fetches it for "me" whenever the server or the signed-in account
// changes, so the stats site's nav and the Admin page agree.

import { createEffect, createRoot, createSignal } from "solid-js";
import { identity } from "../state/identity";
import { live } from "../state/live";

export const adminState = createRoot(() => {
  // `${server base}|${xuid}` -> is that account an admin there (absent:
  // not asked yet).
  const [known, setKnown] = createSignal<Record<string, boolean>>({});
  const key = () => {
    const me = identity.me();
    return me ? `${live.server().base}|${me.xuid}` : null;
  };

  async function check(): Promise<void> {
    const me = identity.me();
    if (!me) return;
    const k = key();
    if (!k || known()[k] !== undefined) return;
    try {
      const r = await fetch(
        `${live.server().api}/accounts/${encodeURIComponent(me.gamertag)}`,
        { headers: { accept: "application/json" } },
      );
      if (!r.ok) return;
      const body = (await r.json()) as { admin?: boolean };
      setKnown({ ...known(), [k]: body.admin === true });
    } catch {
      /* offline; the Admin page shows its own error */
    }
  }

  createEffect(() => {
    void live.server().base;
    void identity.me()?.xuid;
    void check();
  });

  return {
    /** True/false once known; null while unasked (or nobody signed in). */
    isMeAdmin: (): boolean | null => {
      const k = key();
      return k ? (known()[k] ?? null) : null;
    },
    /** After a grant, a server or an account switch: ask again. */
    reset(): void {
      setKnown({});
      void check();
    },
  };
});
