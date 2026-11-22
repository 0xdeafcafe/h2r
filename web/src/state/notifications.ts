import { createEffect, createMemo, createRoot, createSignal, on, onCleanup } from "solid-js";
import { parseSessions, mapName, levelName } from "../lib/sessions";
import {
  changedSession, deliverNotification, joinNotice, JoinTransitions, notificationPermission, NotificationRateLimit,
  notificationSettings, requestNotificationPermission, type LiveNotice, type NotificationSettings,
} from "../lib/notifications";
import { mapTitle } from "./gameart";
import { gameMapTitle } from "./maps";
import { identity } from "./identity";
import { live } from "./live";
import { social } from "./social";

const SETTLE_MS = 250;
const TOAST_MS = 7_000;
const browser = () => typeof Notification === "undefined" ? undefined : Notification;

function gamertag(xuid: string): string | undefined {
  const account = live.state.accounts[xuid];
  return account && !account.bot && account.gamertag !== xuid ? account.gamertag : undefined;
}

export const notifications = createRoot(() => {
  const [settings, setSettings] = createSignal(notificationSettings(null));
  const [permission, setPermission] = createSignal(notificationPermission(browser()));
  const [requesting, setRequesting] = createSignal(false);
  const [toast, setToast] = createSignal<LiveNotice>();
  const account = createMemo(() => {
    const me = identity.me();
    return me ? `h2live.notifications.${JSON.stringify([me.server, me.xuid])}` : null;
  });

  const refreshPermission = () => setPermission(notificationPermission(browser()));
  window.addEventListener("focus", refreshPermission);
  document.addEventListener("visibilitychange", refreshPermission);
  let dismissTimer: ReturnType<typeof setTimeout> | undefined;

  function dismissToast() {
    clearTimeout(dismissTimer);
    setToast(undefined);
  }

  function readSettings(key: string | null) {
    let raw = null;
    try { if (key) raw = localStorage.getItem(key); } catch { /* storage disabled */ }
    setSettings(notificationSettings(raw));
  }

  createEffect(on(account, (key) => readSettings(key)));
  window.addEventListener("storage", (event) => {
    const key = account();
    if (key && (event.key === key || event.key === null)) readSettings(key);
  });

  function toggle(kind: keyof NotificationSettings) {
    const key = account();
    if (!key) return;
    const next = { ...settings(), [kind]: !settings()[kind] };
    setSettings(next);
    try { localStorage.setItem(key, JSON.stringify(next)); } catch { /* storage disabled */ }
  }

  async function requestPermission() {
    if (!account() || requesting() || navigator.userActivation?.isActive === false) return;
    setRequesting(true);
    try { setPermission(await requestNotificationPermission(browser())); }
    finally { setRequesting(false); }
  }

  createEffect(on(account, (key) => {
    if (!key) return;
    const transitions = new JoinTransitions();
    const limit = new NotificationRateLimit();
    let closeNotification: (() => void) | undefined;
    onCleanup(() => { closeNotification?.(); dismissToast(); });

    createEffect(on(settings, () => {
      closeNotification?.();
      dismissToast();
    }, { defer: true }));

    createEffect(on(() => [live.status(), settings()] as const, ([status, enabled]) => {
      if (status !== "live" || (!enabled.friendGames && !enabled.otherGames)) return;
      const me = identity.me();
      if (!me) return;
      const { xuid, server } = me;
      const controller = new AbortController();
      const { signal } = controller;
      const rest = live.api();
      const changed = new Set<string>();
      let refreshTimer: ReturnType<typeof setTimeout> | undefined;
      let reading = false;
      let baselined = false;

      function schedule() {
        if (!reading && refreshTimer === undefined) refreshTimer = setTimeout(() => void refresh(), SETTLE_MS);
      }

      async function refresh() {
        refreshTimer = undefined;
        reading = true;
        const ids = [...changed];
        changed.clear();
        try {
          const page = await rest.sessions(signal);
          if (signal.aborted) return;
          const sessions = parseSessions(page.items);
          const friends = new Map<string, string>(social.mine()?.friends.map((friend) => [friend.xuid, friend.gamertag] as const));
          const audience = { me: xuid, friends, gamertag, settings: enabled };
          if (!baselined) {
            transitions.baseline(sessions, audience);
            baselined = true;
            return;
          }
          const byId = new Map(sessions.map((session) => [session.session_id, session]));
          for (const id of ids) {
            const start = transitions.update(id, byId.get(id), audience);
            if (!start || !limit.allow(performance.now())) continue;
            const session = start.session;
            const map = session.campaign ? levelName(session.campaign, mapTitle)
              : mapName(session, (stem) => gameMapTitle({ map: stem, map_id: session.map_id }));
            const notice = joinNotice(start, map);
            closeNotification?.();
            closeNotification = deliverNotification(
              notice, document.visibilityState, browser(),
              (item) => { dismissToast(); setToast(item); dismissTimer = setTimeout(dismissToast, TOAST_MS); },
              (href) => {
                window.focus();
                const url = new URL(href, location.origin);
                url.searchParams.set("server", server);
                location.assign(url.href);
              },
            );
          }
        } catch {
          // A missing or failed listing cannot establish what became joinable.
          baselined = false;
        } finally {
          reading = false;
          if (!signal.aborted && changed.size) schedule();
        }
      }

      social.mine(); // Start reading the friend list alongside the session baseline.
      const unsubscribe = live.onEvent((event, isLive) => {
        const id = isLive && changedSession(event);
        if (!id) return;
        changed.add(id);
        schedule();
      });
      void refresh();
      onCleanup(() => {
        unsubscribe();
        controller.abort();
        clearTimeout(refreshTimer);
      });
    }));
  }));

  return { settings, toggle, permission, requesting, requestPermission, toast, dismissToast };
});
