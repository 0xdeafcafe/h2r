// What the app reloads when the signed-in account changes: reads depend on
// who reads (a friends-only co-op game, a friend's presence in it), so the
// live connection starts over as the new reader at once. A change of server
// is not a sign-in change: that path already reconnects.

import { createEffect, on, type Accessor } from "solid-js";

export interface Reader { base: string; who: string | null }

/** Whether going from `prev` to `now` is a sign-in or sign-out (the same
 *  server, another reader). The first reading is not. */
export function signInChanged(prev: Reader | undefined, now: Reader): boolean {
  return !!prev && prev.base === now.base && prev.who !== now.who;
}

/** Calls `reload` on each [`signInChanged`] step of `reader`. */
export function onSignInChange(reader: Accessor<Reader>, reload: () => void): void {
  createEffect(on(reader, (now, prev) => { if (signInChanged(prev, now)) reload(); }));
}
