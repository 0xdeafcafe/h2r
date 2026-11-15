// The message of the day as the views show it (no Solid, no fetch: tested
// in test/motd.test.ts). The game shows the header on its Live main menu and
// a random message every 15 s on the matchmaking screen; the dashboard shows
// the header and the stats site cycles the messages.

import type { MotdDoc } from "../admin/validate";

/** The header worth showing, or null when there is none. */
export function motdHeader(doc: MotdDoc | null | undefined): string | null {
  const h = doc?.header?.trim();
  return h ? h : null;
}

/** The messages worth showing: trimmed, blanks dropped. */
export function motdMessages(doc: MotdDoc | null | undefined): string[] {
  return (doc?.messages ?? []).map((m) => m.trim()).filter(Boolean);
}

/** Index `i` moved `d` steps round a ring of `n` (0 when empty). */
export function stepIndex(i: number, d: number, n: number): number {
  if (n <= 0) return 0;
  const at = Math.min(Math.max(0, Math.trunc(i)), n - 1);
  return (((at + d) % n) + n) % n;
}

/** A random place to start in a ring of `n`. */
export function randomStart(n: number, rand: () => number = Math.random): number {
  return n > 0 ? Math.min(n - 1, Math.floor(rand() * n)) : 0;
}
