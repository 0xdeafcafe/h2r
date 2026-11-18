import type { WireEvent } from "../api/types";

// PlayerJoined and PlayerLeft also occur on Live session streams.
// Medals are derived from game kills and have no wire event of their own.
export const STATS_EVENT_KINDS: Readonly<Record<string, { stream: string; category: string }>> = {
  GameStarted: { stream: "game-", category: "game" },
  PlayerJoined: { stream: "game-", category: "game" },
  PlayerLeft: { stream: "game-", category: "game" },
  PlayerKilled: { stream: "game-", category: "kill" },
  ScoreChanged: { stream: "game-", category: "score" },
  GameEnded: { stream: "game-", category: "game" },
  RankChanged: { stream: "rank-", category: "score" },
};

export function isStatsEvent(event: Pick<WireEvent, "type" | "stream">): boolean {
  if (!Object.hasOwn(STATS_EVENT_KINDS, event.type)) return false;
  const kind = STATS_EVENT_KINDS[event.type];
  return !!kind && event.stream.startsWith(kind.stream);
}
