import type { Api } from "../api/http";
import type { WireEvent } from "../api/types";
import { recordedFrame } from "../api/pack";
import { gameTypeLine } from "../state/gametype";
import { gameMapTitle } from "../state/maps";

export const EVENT_WINDOW = 200;

export function eventWindowStart(end: number): number {
  return Math.max(0, end - EVENT_WINDOW);
}

/** Read one range of physical log positions. Hidden events can leave holes,
 *  so callers page by the range boundary, even when it has no visible rows. */
export async function readEventWindow(api: Pick<Api, "events">, end: number, signal?: AbortSignal): Promise<WireEvent[]> {
  const from = eventWindowStart(end);
  if (end <= from) return [];
  const page = await api.events({ from, limit: end - from }, signal);
  return page.items.filter((event) => event.position <= end);
}

export function eventSummary(e: WireEvent): string {
  const d = (e.data ?? {}) as Record<string, unknown>;
  switch (e.type) {
    case "AccountCreated": return `${d.gamertag} signed up`;
    case "SignedIn": return `signed in (${d.state ?? "online"})`;
    case "SignedOut": return "signed out";
    case "PresenceExpired": return "went quiet";
    case "PresenceChanged": return `${d.state ?? ""}${d.game_id ? " in a game" : ""}`;
    case "GameStarted": {
      const s = (k: string) => (typeof d[k] === "string" ? (d[k] as string) : null);
      const players = Array.isArray(d.players) ? d.players : [];
      const type = gameTypeLine({
        mode: s("mode") ?? "", kind: s("kind"), playlist: s("playlist"), online: d.online === true, players: { ...players },
        variant: s("variant"), difficulty: s("difficulty"),
      });
      return `${gameMapTitle({ map: d.map as string, map_id: typeof d.map_id === "string" ? d.map_id : null })} · ${type} · ${players.length} ${players.length === 1 ? "player" : "players"}`;
    }
    case "PlayerKilled": return `${d.killer ?? "world"} → ${d.victim} (${d.kind}${d.weapon ? ", " + d.weapon : ""})`;
    case "PlayerJoined": return String((d.player as { name?: string } | undefined)?.name ?? "");
    case "PlayerLeft": return `slot ${d.index} left`;
    case "RankChanged": return d.playlist === undefined ? "" : `playlist ${d.playlist} · level ${(d.after as { level?: number } | undefined)?.level ?? "–"}`;
    case "ScoreChanged": return `tick ${d.tick}`;
    case "GameFrame": {
      const f = recordedFrame(d);
      return f ? `tick ${f.tick}, ${f.players.length} players` : `tick ${d.tick}`;
    }
    case "GameEnded": {
      const w = d.winner as { team?: number; player?: number } | null | undefined;
      const who = w ? (w.team !== undefined ? `team ${w.team} won` : `slot ${w.player} won`) : "no winner";
      return `${d.reason}, ${who}`;
    }
    default: return "";
  }
}
