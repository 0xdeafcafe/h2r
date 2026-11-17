// The main menu's items. Signed out it leads with the ways in (sign in,
// create, recover), then what works without an account; the items that
// need one show greyed with a hint and open nothing. Signed in: the usual
// items and the account (recovery code, authenticator, devices, sign out).
// Profile and the server live under settings.

export type SectionId = "live" | "sessions" | "friends" | "clans" | "players" | "history" | "settings" | "signup" | "server" | "signin" | "recover" | "account";

export function menuFor(signedIn: boolean): SectionId[] {
  return signedIn
    ? ["live", "sessions", "friends", "players", "history", "account", "settings"]
    : ["signin", "signup", "recover", "live", "history", "sessions", "players", "settings"];
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
