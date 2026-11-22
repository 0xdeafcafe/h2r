import { A } from "@solidjs/router";
import { Show } from "solid-js";
import { campaignLine, gametype, listed, mapName, seats } from "../lib/sessions";
import { mapTitle } from "../state/gameart";
import { notifications } from "../state/notifications";
import { useSessions } from "../state/sessions";
import { Plate } from "./parts";
import "../styles/dash-notifications.css";

export function notificationRows() {
  return [
    { key: "friendGames", label: "games with friends", meta: notifications.settings().friendGames ? "on" : "off" },
    { key: "otherGames", label: "games with other players", meta: notifications.settings().otherGames ? "on" : "off" },
    ...(notifications.permission() === "default" ? [{
      key: "permission", label: notifications.requesting() ? "waiting for browser…" : "allow browser notifications",
    }] : []),
  ];
}

const permissionHelp = {
  default: "Click “allow browser notifications” to get alerts in the background.",
  granted: "Your browser can show alerts while this tab is in the background.",
  denied: "Allow notifications in your browser’s site settings to get alerts in the background.",
  unsupported: "This browser can show alerts here while the tab is visible.",
};

export function NotificationSettingsPlate() {
  return (
    <Plate small="settings" title="join in progress">
      <p>Get alerts here when players enter games or lobbies you can join.</p>
      <p role="status">{permissionHelp[notifications.permission()]}</p>
    </Plate>
  );
}

export function NotificationToast() {
  return (
    <div class="dx-notice-region" aria-live="polite" aria-atomic="true">
      <Show when={notifications.toast()}>
        {(notice) => (
          <div class="dx-notice">
            <A class="dx-notice-link" href={notice().href} onClick={notifications.dismissToast}>
              <strong>{notice().title}</strong>
              <span>{notice().body}</span>
            </A>
            <button type="button" class="dx-notice-dismiss" aria-label="Dismiss notification" onClick={notifications.dismissToast}>×</button>
          </div>
        )}
      </Show>
    </div>
  );
}

export function NotificationSession(props: { id?: string }) {
  const sessions = useSessions();
  const session = () => sessions.items().find((s) => s.session_id === props.id && listed(s));
  const unavailable = () => {
    switch (sessions.status()) {
      case "loading": return "Loading the game session…";
      case "error": return "Couldn’t load the game session.";
      default: return "This game session is no longer available.";
    }
  };

  return (
    <Show when={session()} fallback={<Plate small="live now" title="game session" main><p>{unavailable()}</p></Plate>}>
      {(item) => (
        <Plate small="game session" title={gametype(item())} main>
          <dl>
            <dt>host</dt><dd>{item().host_gamertag || "dedicated server"}</dd>
            <dt>map</dt><dd>{campaignLine(item(), mapTitle) ?? mapName(item(), mapTitle)}</dd>
            <dt>players</dt><dd>{seats(item()).filled} of {seats(item()).max}</dd>
          </dl>
        </Plate>
      )}
    </Show>
  );
}
