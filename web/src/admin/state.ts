// Whether the signed-in browser account is an admin on the current server.
// The account read API carries `admin: true` for admins (docs/live/live-admin.md);
// this fetches it for "me" whenever the server or the signed-in account
// changes, so the stats site's nav and the Admin page agree.

import { createEffect, createRoot, createSignal } from "solid-js";
import { identity } from "../state/identity";
import { live } from "../state/live";
import type { InviteGrant } from "../api/types";

export const adminState = createRoot(() => {
  // `${server base}|${xuid}` -> the signed-in account's roles there.
  const [known, setKnown] = createSignal<Record<string, { admin: boolean; invite_grant: InviteGrant | null }>>({});
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
      const body = await live.api().account(me.gamertag);
      setKnown({ ...known(), [k]: { admin: body.admin === true, invite_grant: body.invite_grant ?? null } });
    } catch {
      /* offline; the Admin page shows its own error */
    }
  }

  createEffect(() => {
    void live.server().base;
    void live.wellKnown();
    void identity.me()?.xuid;
    void check();
  });

  return {
    /** True/false once known; null while unasked (or nobody signed in). */
    isMeAdmin: (): boolean | null => {
      const k = key();
      return k ? (known()[k]?.admin ?? null) : null;
    },
    myInviteGrant(): InviteGrant | null {
      const me = identity.me(), k = key();
      if (!me || !k) return null;
      const streamed = live.state.accounts[me.xuid]?.invite_grant;
      return streamed === undefined ? known()[k]?.invite_grant ?? null : streamed;
    },
    /** After a grant, a server or an account switch: ask again. */
    reset(): void {
      setKnown({});
      void check();
    },
  };
});
