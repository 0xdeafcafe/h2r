import type { WireEvent } from "../api/types";

// PlayerJoined and PlayerLeft also occur on Live session streams.
// GameMedal is one of Halo 2's own medals, as the host reports it. Our modes'
// medals (the bounce family, Incel) have no wire event: Live works them out
// from the bounce and teabag events and serves them with the game reads and
// the stats.
export const STATS_EVENT_KINDS: Readonly<Record<string, { stream: string; category: string }>> = {
  GameStarted: { stream: "game-", category: "game" },
  PlayerJoined: { stream: "game-", category: "game" },
  PlayerLeft: { stream: "game-", category: "game" },
  PlayerKilled: { stream: "game-", category: "kill" },
  Superbounce: { stream: "game-", category: "bounce" },
  Bounced: { stream: "game-", category: "bounce" },
  Teabag: { stream: "game-", category: "kill" },
  GameMedal: { stream: "game-", category: "medal" },
  ScoreChanged: { stream: "game-", category: "score" },
  GameEnded: { stream: "game-", category: "game" },
  RankChanged: { stream: "rank-", category: "score" },
};

export function isStatsEvent(event: Pick<WireEvent, "type" | "stream">): boolean {
  if (!Object.hasOwn(STATS_EVENT_KINDS, event.type)) return false;
  const kind = STATS_EVENT_KINDS[event.type];
  return !!kind && event.stream.startsWith(kind.stream);
}
