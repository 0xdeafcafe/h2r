// The main menu's items. Signed out it leads with the ways in (sign in,
// create, recover), then what works without an account; the items that
// need one show greyed with a hint and open nothing. Signed in: the usual
// items and the account (recovery code, authenticator, devices, sign out).
// Profile and the server live under settings.

import { LIVE_ADMIN, statsPath } from "../routes";
import type { GlyphName } from "./parts";

export type SectionId = "live" | "games" | "sessions" | "friends" | "teams" | "players" | "settings" | "signup" | "server" | "signin" | "recover" | "account";

export function menuFor(signedIn: boolean): SectionId[] {
  return signedIn
    ? ["live", "games", "sessions", "friends", "players", "account", "settings"]
    : ["signin", "signup", "recover", "live", "games", "sessions", "players", "settings"];
}

/** Why a menu item is greyed: it needs an account. Null when it opens. */
export function lockedHint(id: SectionId, signedIn: boolean): string | null {
  if (signedIn) return null;
  switch (id) {
    case "sessions": return "sign in to find a game";
    case "players": return "sign in to see players";
    case "settings": return "sign in for settings";
    default: return null;
  }
}

export function settingsRows(signedIn: boolean, admin: boolean | null): { key: string; label: string; glyph: GlyphName }[] {
  return [
    { key: "/dash/signup", label: signedIn ? "profile" : "sign up", glyph: "key" },
    { key: "/dash/games", label: "games", glyph: "disc" },
    { key: admin === true ? `${LIVE_ADMIN}/events` : statsPath("/events"), label: "event log", glyph: "history" },
    { key: "/dash/server", label: "server", glyph: "server" },
    ...(admin === true ? [{ key: LIVE_ADMIN, label: "live admin", glyph: "gear" as const }] : []),
  ];
}
