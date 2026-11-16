// The stats site's admin area keeps only Halo's own things: what the game
// shows and plays. Service administration (invites, accounts and admins,
// importing an account, the server itself) is on the dashboard's server
// screens (src/dash/Server.tsx).

export interface AdminTab { id: "motd" | "hopper" | "content"; label: string }

export const ADMIN_TABS: readonly AdminTab[] = [
  { id: "motd", label: "Message of the Day" },
  { id: "hopper", label: "Playlists" },
  { id: "content", label: "Custom content" },
];

/** The tab a `?tab=` asks for; the first one for anything else (an old
 *  `?tab=import` link included, now a dashboard screen). */
export function adminTab(asked: string | null): AdminTab["id"] {
  return ADMIN_TABS.find((t) => t.id === asked)?.id ?? "motd";
}

/** Where the stats admin sends service work. */
export const SERVER_ADMIN = "/dash/server";
export const IMPORT_ACCOUNT = "/dash/server/import";
