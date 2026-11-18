// Initial load: REST snapshots, joined to the stream by log position
// (contract clarification 2). The stream then resumes after the oldest
// snapshot position, and the reducer tolerates the overlap.

import type { AccountSummary, GameDetail, GameSummary, PresenceEntry, WireEvent } from "../api/types";
import type { Api } from "../api/http";

export interface Snapshot {
  /** Stream from here (exclusive). */
  position: number;
  accounts: AccountSummary[];
  presence: PresenceEntry[];
  games: Array<GameSummary | GameDetail>;
  /** Recent raw events for the event page (before `position`). */
  tail: WireEvent[];
}

/** How many ended games get a full detail fetch (scoreboard, kills). */
export const ENDED_DETAILS = 40;
const TAIL_EVENTS = 300;

async function pool<T, R>(items: T[], n: number, f: (t: T) => Promise<R>): Promise<R[]> {
  const out: R[] = new Array(items.length);
  let i = 0;
  await Promise.all(Array.from({ length: Math.min(n, items.length) }, async () => {
    while (i < items.length) { const k = i++; out[k] = await f(items[k]!); }
  }));
  return out;
}

/** The games a snapshot holds: the details of live games and the newest
 *  ended ones where there is one, summaries for the rest. */
function joinGames(live: GameSummary[], ended: GameSummary[], detail: (g: GameSummary, i: number) => GameDetail | null) {
  const wanted = [...live, ...ended.slice(0, ENDED_DETAILS)];
  const games: Array<GameSummary | GameDetail> = [...ended.slice(ENDED_DETAILS)];
  const details: GameDetail[] = [];
  wanted.forEach((g, i) => {
    const d = detail(g, i);
    games.push(d ?? g);
    if (d) details.push(d);
  });
  return { wanted, games, details };
}

/** Returns null if the server doesn't report positions (then the caller
 *  replays the whole log instead). One round trip (`GET /v1/snapshot`)
 *  where the server has it; else a list each, a detail a game and a page
 *  of the log. */
export async function loadSnapshot(a: Api, signal?: AbortSignal): Promise<Snapshot | null> {
  try {
    const b = await a.snapshot(signal);
    const byId = new Map(b.details.map((d) => [d.game_id, d]));
    const { games } = joinGames(b.live.items, b.ended.items, (g) => byId.get(g.game_id) ?? null);
    return { position: b.position, accounts: b.accounts.items, presence: b.presence.items, games, tail: b.tail };
  } catch (e) {
    if (signal?.aborted) throw e;
    // An older server: the reads one by one.
  }
  return loadSnapshotByParts(a, signal);
}

async function loadSnapshotByParts(a: Api, signal?: AbortSignal): Promise<Snapshot | null> {
  const [accounts, presence, live, ended] = await Promise.all([
    a.accounts(signal), a.presence(signal), a.games("live", 100, signal), a.games("ended", 100, signal),
  ]);
  const positions: Array<number | null> = [accounts.position, presence.position, live.position, ended.position];
  const wanted = [...live.items, ...ended.items.slice(0, ENDED_DETAILS)];
  const details = await pool(wanted, 6, (g) => a.game(g.game_id, signal).catch(() => null));
  const games: Array<GameSummary | GameDetail> = [...ended.items.slice(ENDED_DETAILS)];
  wanted.forEach((g, i) => {
    const d = details[i];
    games.push(d ?? g);
    if (d) positions.push(d.position ?? null);
  });
  if (positions.some((p) => p === null)) return null;
  const position = Math.min(...(positions as number[]));
  let tail: WireEvent[] = [];
  try {
    // Without the 1 Hz GameFrames, which the stream leaves out too (they
    // were nine tenths of this page's bytes).
    tail = (await a.events({ from: Math.max(0, position - TAIL_EVENTS), limit: TAIL_EVENTS, exclude: "GameFrame" }, signal)).items
      .filter((e) => e.position <= position);
  } catch { /* the log page just starts empty */ }
  return { position, accounts: accounts.items, presence: presence.items, games, tail };
}

/** The games among `ids` (held back from the stream: lib reducer
 *  `needsConfirm`) that the reader's own `GET /v1/games/{id}` returns. One
 *  that answers 404, or fails, is left out: it shows only if a later
 *  filtered list has it. */
export async function confirmGames(ids: readonly string[], read: (id: string) => Promise<GameDetail>): Promise<GameDetail[]> {
  const got = await Promise.all(ids.map((id) => read(id).then((d) => d, () => null)));
  return got.filter((d): d is GameDetail => d !== null);
}
