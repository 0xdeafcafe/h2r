import type { FilmFront } from "../api/host-film";
import type { GamePlayer } from "../api/types";
import { applyGameSnapshot, initialState, type GameState } from "../state/reducer";

/** A catalogue film can precede its stats ledger. Keep this display state
 * local to the authorised Theater route and address ranges by the lease ID. */
export function filmGame(id: string, front: FilmFront): GameState {
  const {summary} = front;
  const players = new Map<number, GamePlayer>();
  for (const p of front.roster) players.set(p.seat, {
    index: p.seat, name: p.name, team: p.team, bot: p.kind === "bot", local: p.kind === "local",
  });
  for (const p of summary.players) if (!players.has(p.seat)) {
    players.set(p.seat, {index: p.seat, name: p.name, team: p.team});
  }
  const campaign = summary.mode === "coop_campaign" || summary.mode === "campaign";
  const state = initialState();
  state.games = Object.create(null);
  const duration = 1000 * summary.duration_ticks / (front.ticks_per_second ?? 30);
  applyGameSnapshot(state, {
    game_id: id, map: summary.level, variant: summary.variant_name || summary.game_type,
    mode: campaign ? "campaign" : summary.mode === "unknown" ? "unknown" : "multiplayer",
    kind: campaign ? "campaign" : summary.mode === "system_link" ? "custom" : summary.mode,
    started_at: new Date(front.started_ms).toISOString(),
    ended_at: new Date(front.started_ms + duration).toISOString(),
    players: [...players.values()],
  });
  const game = state.games[id]!;
  // A menu row gives a final score. Kills, deaths and assists belong to
  // recorded ScoreChanged events and can be absent from an incomplete film.
  game.scores = {};
  return game;
}
