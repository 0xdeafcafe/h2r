import { describe, expect, test } from "bun:test";
import { settingsRows } from "../src/dash/menu";
import { parseSession, type Session } from "../src/lib/sessions";
import {
  changedSession, deliverNotification, joinNotice, JoinTransitions, notificationPermission,
  NotificationRateLimit, notificationSettings, requestNotificationPermission, type LiveNotice,
} from "../src/lib/notifications";

const session = (patch: Partial<Session> = {}): Session => ({
  ...parseSession({
    session_id: "0000000000000001",
    host_xuid: "host",
    host_gamertag: "afr",
    advertisement: {
      name: "Slayer",
      map: "lockout",
      slots: { public_filled: 3, public_open: 5 },
    },
  })!,
  ...patch,
});

const viewer = {
  me: "me",
  friends: new Map([["host", "afr"], ["friend", "sneaky pete"]]),
  gamertag: (xuid: string) => xuid === "stranger" ? "other player" : undefined,
  settings: { friendGames: true, otherGames: false },
};

describe("join-in-progress transitions", () => {
  test("a new joinable advertisement produces a notice with the session detail as its action", () => {
    const tracker = new JoinTransitions();
    tracker.baseline([], viewer);
    const lobby = session();
    const start = tracker.update(lobby.session_id, lobby, viewer);
    expect(start).toBeDefined();
    expect(joinNotice(start!, "Lockout")).toEqual({
      id: "session-0000000000000001",
      title: "afr is playing Slayer on Lockout",
      body: "3 of 8 · join now",
      href: "/dash/session/0000000000000001",
    });
  });

  test("a friend arriving in a joinable roster is a start even when the host is a stranger", () => {
    const tracker = new JoinTransitions();
    const lobby = session({ host_xuid: "stranger", host_gamertag: "player" });
    tracker.baseline([lobby], viewer);
    const joined = { ...lobby, players: { friend: "public" } };
    expect(tracker.update(lobby.session_id, joined, viewer)?.gamertag).toBe("sneaky pete");
    expect(tracker.update(lobby.session_id, lobby, viewer)).toBeUndefined();
    expect(tracker.update(lobby.session_id, joined, viewer)).toBeUndefined();
  });

  test("the other-players toggle can alert for a nonfriend arriving in a friend's lobby", () => {
    const tracker = new JoinTransitions();
    const audience = { ...viewer, settings: { friendGames: false, otherGames: true } };
    const lobby = session();
    tracker.baseline([lobby], audience);
    const joined = { ...lobby, players: { stranger: "public" } };
    expect(tracker.update(lobby.session_id, joined, audience)?.gamertag).toBe("other player");
  });

  test("a seat opening up is a start; reserved seats alone do not qualify", () => {
    const tracker = new JoinTransitions();
    const full = session({ slots: { public_filled: 7, public_open: 0, private_filled: 0, private_open: 1 } });
    tracker.baseline([full], viewer);
    expect(tracker.update(full.session_id, full, viewer)).toBeUndefined();
    expect(tracker.update(full.session_id, session(), viewer)?.gamertag).toBe("afr");
  });

  test("separate toggles, visibility and current membership control eligibility", () => {
    const others = { ...viewer, settings: { friendGames: false, otherGames: true } };
    for (const [patch, audience, eligible] of [
      [{}, viewer, true],
      [{ visibility: "friends" }, viewer, true],
      [{ visibility: "invite" }, viewer, false],
      [{ visibility: "friends", host_xuid: "stranger" }, others, false],
      [{ host_xuid: "stranger" }, viewer, false],
      [{ host_xuid: "stranger" }, others, true],
      [{}, others, false],
      [{ host_xuid: "me" }, viewer, false],
      [{ players: { me: "public" } }, viewer, false],
      [{ state: "expired" }, viewer, false],
    ] satisfies [Partial<Session>, typeof viewer, boolean][]) {
      const tracker = new JoinTransitions();
      const lobby = session(patch);
      expect(!!tracker.update(lobby.session_id, lobby, audience)).toBe(eligible);
    }
  });

  test("initial and reconnect baselines absorb existing games and preserve deduplication", () => {
    const tracker = new JoinTransitions();
    const existing = session();
    tracker.baseline([existing], viewer);
    expect(tracker.update(existing.session_id, existing, viewer)).toBeUndefined();
    const next = session({ session_id: "0000000000000002" });
    expect(tracker.update(next.session_id, next, viewer)).toBeDefined();
    const missed = session({ session_id: "0000000000000003" });
    tracker.baseline([missed], viewer);
    expect(tracker.update(missed.session_id, missed, viewer)).toBeUndefined();
    expect(tracker.update(next.session_id, next, viewer)).toBeUndefined();
  });

  test("the session stream distinguishes roster events from telemetry and mailbox invitations", () => {
    expect(changedSession({ type: "SessionCreated", stream: "session-abc" })).toBe("abc");
    expect(changedSession({ type: "PlayerJoined", stream: "session-abc" })).toBe("abc");
    expect(changedSession({ type: "PlayerJoined", stream: "game-abc" })).toBeUndefined();
    expect(changedSession({ type: "GameInviteSent", stream: "mailbox-me" })).toBeUndefined();
  });
});

const notice: LiveNotice = {
  id: "session-abc", title: "afr is playing Slayer on Lockout",
  body: "3 of 8 · join now", href: "/dash/session/abc",
};

class FakeNotification {
  static permission: NotificationPermission = "default";
  static answer: NotificationPermission = "granted";
  static prompts = 0;
  static shown: FakeNotification[] = [];

  static async requestPermission(): Promise<NotificationPermission> {
    this.prompts++;
    return this.permission = this.answer;
  }

  onclick: Notification["onclick"] = null;
  closed = false;

  constructor(readonly title: string, readonly options: NotificationOptions) {
    FakeNotification.shown.push(this);
  }

  close() { this.closed = true; }
  click() { if (this.onclick) Reflect.apply(this.onclick, this, []); }
}

describe("browser notification preferences and delivery", () => {
  test("settings defaults are off and the notification row is on the signed-in dashboard", () => {
    expect(notificationSettings(null)).toEqual({ friendGames: false, otherGames: false });
    expect(notificationSettings("{invalid")).toEqual({ friendGames: false, otherGames: false });
    expect(notificationSettings('{"friendGames":true,"otherGames":false}')).toEqual({ friendGames: true, otherGames: false });
    expect(settingsRows(true, false).find((row) => row.label === "notifications")?.key).toBe("/dash/settings/notifications");
    expect(settingsRows(false, false).some((row) => row.label === "notifications")).toBe(false);
  });

  test("reading permission never prompts and only default can request it", async () => {
    FakeNotification.permission = "default";
    FakeNotification.answer = "granted";
    FakeNotification.prompts = 0;
    expect(notificationPermission(FakeNotification)).toBe("default");
    expect(FakeNotification.prompts).toBe(0);
    expect(await requestNotificationPermission(FakeNotification)).toBe("granted");
    expect(await requestNotificationPermission(FakeNotification)).toBe("granted");
    expect(FakeNotification.prompts).toBe(1);

    FakeNotification.permission = "default";
    FakeNotification.answer = "denied";
    expect(await requestNotificationPermission(FakeNotification)).toBe("denied");
    expect(await requestNotificationPermission(FakeNotification)).toBe("denied");
    expect(FakeNotification.prompts).toBe(2);
    expect(await requestNotificationPermission(undefined)).toBe("unsupported");
  });

  test("visible tabs use a toast and hidden tabs require permission for browser delivery", () => {
    const toasts: LiveNotice[] = [];
    const opened: string[] = [];
    const deliver = (visibility: DocumentVisibilityState) =>
      deliverNotification(notice, visibility, FakeNotification, (toast) => toasts.push(toast), (href) => opened.push(href));
    FakeNotification.shown = [];
    FakeNotification.permission = "granted";
    deliver("visible");
    expect(toasts).toEqual([notice]);
    expect(FakeNotification.shown).toHaveLength(0);

    FakeNotification.permission = "default";
    deliver("hidden");
    FakeNotification.permission = "denied";
    deliver("hidden");
    expect(FakeNotification.shown).toHaveLength(0);

    FakeNotification.permission = "granted";
    deliver("hidden");
    expect(toasts).toEqual([notice]);
    expect(FakeNotification.shown).toHaveLength(1);
    const posted = FakeNotification.shown[0]!;
    expect(posted.title).toBe(notice.title);
    expect(posted.options).toEqual({ body: notice.body, tag: notice.id });
    posted.click();
    expect(posted.closed).toBe(true);
    expect(opened).toEqual([notice.href]);
  });

  test("a burst can display at most one alert every eight seconds", () => {
    const limit = new NotificationRateLimit();
    expect(limit.allow(20)).toBe(true);
    expect(limit.allow(21)).toBe(false);
    expect(limit.allow(8_019)).toBe(false);
    expect(limit.allow(8_020)).toBe(true);
  });
});
