// Links carry only the selected Live server and its advertised session ID.
import { seats, type Session } from "./sessions";

export function joinHref(server: string, session: string): string | null {
  if (session.length !== 16 || !/^[0-9a-f]{16}$/.test(session)) return null;
  // Match the native parser before URL normalizes an empty authority, userinfo,
  // dot segment, or other spelling that the link protocol rejects.
  const parts = /^https?:\/\/([^/]+)(?:\/([A-Za-z0-9._~/-]*))?$/i.exec(server);
  if (!parts || /[^!-~]|[\\?#]/.test(server) || /[@%]/.test(parts[1]!)
    || parts[2]?.split("/").some((s) => s === "." || s === "..")) return null;
  let base: URL;
  try { base = new URL(server); } catch { return null; }
  const query = new URLSearchParams({ server: base.href.replace(/\/+$/, ""), session });
  const link = `h2recomp://join?${query}`;
  return link.length <= 4096 ? link : null;
}

/** Receives advertisements already filtered by the Live server for this reader. */
export function canJoinSession(s: Session, viewer?: string): boolean {
  return !!(s.host_gamertag || s.host_lease) && s.state === "open" && s.visibility !== "invite" && !s.assigned
    && (s.flags === "custom" || s.flags === "coop")
    && s.slots.public_open > 0 && seats(s).open > 0
    && (!viewer || (viewer !== s.host_xuid && !Object.hasOwn(s.players, viewer)));
}

export function friendSession(sessions: Session[], friend: string, viewer: string): Session | undefined {
  const matches = sessions.filter((s) => s.state === "open" && (s.host_xuid === friend || Object.hasOwn(s.players, friend)));
  // If two hosts report the same friend, their presence cannot identify which lobby to open.
  const session = matches.length === 1 ? matches[0] : undefined;
  return session && canJoinSession(session, viewer) ? session : undefined;
}

/** Browsers don't report whether a scheme has a handler. Leaving the page
 *  cancels the hint; staying here long enough offers the installation guide. */
export function watchLaunch(page: EventTarget & { readonly hidden: boolean }, show: () => void, delay = 1800): () => void {
  function cancel() {
    clearTimeout(timer);
    page.removeEventListener("visibilitychange", visibility);
  }
  function visibility() { if (page.hidden) cancel(); }
  const timer = setTimeout(() => {
    cancel();
    if (!page.hidden) show();
  }, delay);
  page.addEventListener("visibilitychange", visibility);
  return cancel;
}
