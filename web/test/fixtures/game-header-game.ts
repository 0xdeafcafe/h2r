// The fixture game behind the game-header fixtures: a finished team Slayer
// with two teams, full and partial appearances, a bot, and a name that needs escaping. The generator
// (gen-game-header.ts) writes its model and markup to game-header.json and
// game-header.html; the Rust side (crates/live-server/src/page_header.rs)
// renders the same model and must write the same markup.

import type { GamePlayerState, GameState } from "../../src/state/reducer";

const player = (p: Partial<GamePlayerState> & Pick<GamePlayerState, "index" | "name" | "team">): GamePlayerState => ({
  xuid: null, local: false, game_name: null, left: false, appearance: null, ...p,
});

export function fixtureGame(): GameState {
  return {
    game_id: "0123abcd",
    console: null,
    host: null,
    map: "orchard",
    map_id: null,
    mode: "multiplayer",
    kind: "custom",
    playlist: null,
    playlist_id: null,
    difficulty: null,
    online: null,
    skulls: [],
    variant: "Team Slayer",
    team_play: true,
    score_limit: 50,
    time_limit_s: 600,
    started_at: "2026-11-18T20:00:00.000Z",
    ended_at: "2026-11-18T20:11:30.000Z",
    status: "ended",
    end_reason: null,
    winner: { team: 1 },
    start_tick: 0,
    players: {
      0: player({ index: 0, name: "afr", xuid: "0009000000000042", team: 0, local: true, appearance: {
        emblem: { fg: 30, bg: 7 }, colours: { primary: 3, secondary: 6, tertiary: 8, quaternary: 2 },
      } }),
      1: player({ index: 1, name: "Sneaky Pete", xuid: "0009000000000043", team: 1, appearance: { colours: { primary: 7 } } }),
      2: player({ index: 2, name: "Clanker-7", team: 1, bot: true }),
      3: player({ index: 3, name: "Rock & Roll", team: 0 }),
    },
    scores: {
      0: { index: 0, score: 15, kills: 14, deaths: 9, assists: 2 },
      1: { index: 1, score: 12, kills: 11, deaths: 10, assists: 1 },
      2: { index: 2, score: 8, kills: 8, deaths: 8, assists: 0 },
      3: { index: 3, score: 3, kills: 3, deaths: 12, assists: 0 },
    },
    teams: { 0: 18, 1: 20 },
    kills: [],
    last_tick: 0,
    first_position: 0,
    positions: [],
  };
}
