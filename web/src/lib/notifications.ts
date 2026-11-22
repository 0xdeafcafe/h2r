import type { WireEvent } from "../api/types";
import { gametype, seats, type Session } from "./sessions";

export interface NotificationSettings {
  friendGames: boolean;
  otherGames: boolean;
}

export function notificationSettings(raw: string | null): NotificationSettings {
  try {
    const saved = JSON.parse(raw ?? "null");
    return { friendGames: saved?.friendGames === true, otherGames: saved?.otherGames === true };
  } catch {
    return { friendGames: false, otherGames: false };
  }
}

interface Audience {
  me: string;
  friends: ReadonlyMap<string, string>;
  gamertag(xuid: string): string | undefined;
  settings: NotificationSettings;
}

interface JoinStart {
  session: Session;
  gamertag: string;
}

function joinable(session: Session, audience: Audience): boolean {
  return session.state === "open" && session.slots.public_open > 0 && seats(session).open > 0
    && session.host_xuid !== audience.me && !Object.hasOwn(session.players, audience.me)
    && (session.visibility === "public"
      || (session.visibility === "friends" && audience.friends.has(session.host_xuid)));
}

const members = (session: Session) => new Set([session.host_xuid, ...Object.keys(session.players)]);

function arrivingPlayer(
  session: Session,
  current: ReadonlySet<string>,
  prior: ReadonlySet<string> | undefined,
  audience: Audience,
): string | undefined {
  const { friends, settings } = audience;
  let other: string | undefined;
  for (const xuid of current) {
    if (prior?.has(xuid)) continue;
    const friend = friends.get(xuid);
    if (friend && settings.friendGames) return friend;
    if (!friend && settings.otherGames && !other) {
      other = (xuid === session.host_xuid ? session.host_gamertag : audience.gamertag(xuid)) || undefined;
    }
  }
  return other;
}

/** Session advertisements and their rosters say where a player can be
 *  joined. Presence game IDs name telemetry, which can also be solo or full. */
export class JoinTransitions {
  private prior = new Map<string, ReadonlySet<string>>();
  private seen = new Set<string>();

  /** An initial or reconnect read establishes a silent baseline. */
  baseline(sessions: readonly Session[], audience: Audience): void {
    this.prior.clear();
    for (const session of sessions) {
      if (!joinable(session, audience)) continue;
      const current = members(session);
      this.prior.set(session.session_id, current);
      if (arrivingPlayer(session, current, undefined, audience)) this.seen.add(session.session_id);
    }
  }

  update(id: string, session: Session | undefined, audience: Audience): JoinStart | undefined {
    if (!session || !joinable(session, audience)) {
      this.prior.delete(id);
      return;
    }
    const prior = this.prior.get(id);
    const current = members(session);
    this.prior.set(id, current);
    if (this.seen.has(id)) return;
    const gamertag = arrivingPlayer(session, current, prior, audience);
    if (!gamertag) return;
    this.seen.add(id);
    return { session, gamertag };
  }
}

/** The listing is the authority for visibility and endpoint eligibility.
 *  The stream tells us which listing entries changed. */
export function changedSession(event: Pick<WireEvent, "stream" | "type">): string | undefined {
  if (!event.stream.startsWith("session-")) return;
  switch (event.type) {
    case "SessionCreated":
    case "SessionUpdated":
    case "SessionDeleted":
    case "SessionExpired":
    case "PlayerJoined":
    case "PlayerLeft":
      return event.stream.slice("session-".length);
  }
}

/** Notification action for a joinable session. The detail page is the
 *  entry point for the future Halo 2 join URL. */
export function notificationAction(session: Pick<Session, "session_id">): string {
  return `/dash/session/${encodeURIComponent(session.session_id)}`;
}

export interface LiveNotice {
  id: string;
  title: string;
  body: string;
  href: string;
}

export function joinNotice({ session, gamertag }: JoinStart, map: string): LiveNotice {
  const { filled, max } = seats(session);
  return {
    id: `session-${session.session_id}`,
    title: session.map || session.campaign
      ? `${gamertag} is playing ${gametype(session)} on ${map}`
      : `${gamertag} has a joinable lobby`,
    body: `${filled} of ${max} · join now`,
    href: notificationAction(session),
  };
}

export interface NotificationBrowser {
  readonly permission: NotificationPermission;
  requestPermission(): Promise<NotificationPermission>;
  new(title: string, options: NotificationOptions): Pick<Notification, "onclick" | "close">;
}

export function notificationPermission(browser: NotificationBrowser | undefined): NotificationPermission | "unsupported" {
  return browser?.permission ?? "unsupported";
}

/** The settings button calls this directly, during its activation. */
export async function requestNotificationPermission(browser: NotificationBrowser | undefined) {
  if (browser?.permission !== "default") return notificationPermission(browser);
  try {
    return await browser.requestPermission();
  } catch {
    return notificationPermission(browser);
  }
}

export function deliverNotification(
  notice: LiveNotice,
  visibility: DocumentVisibilityState,
  browser: NotificationBrowser | undefined,
  showToast: (notice: LiveNotice) => void,
  open: (href: string) => void,
): (() => void) | undefined {
  if (visibility === "visible") {
    showToast(notice);
    return;
  }
  if (browser?.permission !== "granted") return;
  try {
    const posted = new browser(notice.title, { body: notice.body, tag: notice.id });
    posted.onclick = () => {
      posted.close();
      open(notice.href);
    };
    return () => { posted.onclick = null; posted.close(); };
  } catch {
    // Some browsers expose Notification but only allow service worker delivery.
    return;
  }
}

export class NotificationRateLimit {
  private next = 0;

  allow(now: number): boolean {
    if (now < this.next) return false;
    this.next = now + 8_000;
    return true;
  }
}
