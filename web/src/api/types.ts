// Wire types for the h2 live service, following docs/live-api.md.
// Optional fields are the ones the doc marks with `?`.

export type Xuid = string;
export type GameId = string;

export interface WireEvent<T = unknown> {
  position: number;
  stream: string;
  version: number;
  type: string;
  at: string;
  data: T;
  author?: string;
  kid?: string;
  jws?: string;
  /** Element of a batch JWS (an array payload) this event is. */
  jws_index?: number;
  hash?: string;
}

export interface GamePlayer {
  index: number;
  name: string;
  xuid?: Xuid | null;
  team: number;
  local?: boolean;
  /** Emblem and colours read from the game (palette indices), when known. */
  appearance?: import("../lib/gameart").Appearance | null;
}

export interface FramePlayer {
  index: number;
  alive: boolean;
  pos: [number, number, number];
  yaw: number;
  vehicle?: string | null;
  /** The player's biped: "spartan" or "elite" (others draw as a spartan). */
  species?: string | null;
  /** Unit aim direction in game coordinates (z up). */
  aim?: [number, number, number] | null;
  /** The held weapon's tag stem, lower case: "battle_rifle", "smg". */
  weapon?: string | null;
  /** Shots the held weapon fired since this player's previous frame. */
  shots?: number | null;
}

export interface Frame {
  game_id: GameId;
  tick: number;
  players: FramePlayer[];
}

export interface PlayerScore {
  index: number;
  score: number;
  kills: number;
  deaths: number;
  assists: number;
}

export interface TeamScore {
  team: number;
  score: number;
}

export type KillKind = "kill" | "suicide" | "betrayal" | "world";

// Event payloads.
export interface AccountCreated { xuid: Xuid; gamertag: string }
export interface GamertagReserved { xuid: Xuid }
export interface TitleFilePublished { title_id: string; name: string; sha256: string; size: number }
export interface PresenceData { xuid: Xuid; state?: string; game_id?: GameId | null }
export interface GameStarted {
  game_id: GameId;
  /** The author who started the game, `acct:<xuid>@<server>` (written by the server). */
  host?: string;
  /** The console's own name for itself, if it gave one. */
  console?: string;
  map: string;
  mode: "campaign" | "multiplayer" | string;
  variant?: string | null;
  team_play: boolean;
  score_limit?: number | null;
  time_limit_s?: number | null;
  /** Game tick the game started at. */
  tick?: number | null;
  players: GamePlayer[];
}
export interface PlayerJoined { player: GamePlayer }
export interface PlayerLeft { index: number }
export interface PlayerKilled {
  tick: number;
  /** The victim's position when it died. */
  pos?: [number, number, number] | null;
  victim: number;
  killer?: number | null;
  weapon?: string | null;
  kind: KillKind;
}
export interface ScoreChanged { tick: number; players: PlayerScore[]; teams?: TeamScore[] | null }
/** `{team}` in team games, `{player}` (slot index) otherwise, null for a draw or quit. */
export type Winner = { team: number } | { player: number };

export interface GameEnded {
  tick: number;
  reason: "finished" | "quit" | "lost" | string;
  winner?: Winner | null;
  /** The final scoreboard. */
  players?: PlayerScore[] | null;
}

export type StreamMessage =
  | { kind: "event"; event: WireEvent }
  | { kind: "frame"; frame: Frame }
  | { kind: "caught_up"; position: number };

// REST shapes.
/** Every list: `next` is an opaque cursor (pass back as `?cursor=`),
 *  `position` the log head when it was read. */
export interface ListResponse<T> { items: T[]; next: string | null; position: number }

export interface AccountSummary { xuid: Xuid; gamertag: string; created_at?: string; online?: boolean; admin?: boolean }

export interface PresenceEntry { xuid: Xuid; gamertag: string; state: string; game_id?: GameId | null; since: string }

export interface GameSummary {
  game_id: GameId;
  map: string;
  mode: string;
  variant?: string | null;
  started_at: string;
  ended_at?: string | null;
  players: GamePlayer[];
  teams?: TeamScore[] | null;
}

export interface KillRecord extends PlayerKilled { position?: number; at?: string }

/** `GET /v1/games/{id}`: the summary plus scoreboard and the last 50 kills.
 *  Fields past the summary are read when present. */
export interface GameDetail extends GameSummary {
  position?: number;
  scoreboard?: PlayerScore[];
  kills?: KillRecord[];
  team_play?: boolean;
  score_limit?: number | null;
  time_limit_s?: number | null;
  console?: string | null;
  host?: string | null;
  tick?: number | null;
  last_tick?: number | null;
  winner?: Winner | null;
  end_reason?: string | null;
}

export interface AccountKeyEntry {
  kid: string;
  jwk: { kty: string; crv: string; x: string; kid?: string };
  valid_from: number | null;
  valid_to: number | null;
}

export interface MapLayer { z: [number, number]; polygons: [number, number][][] }
export interface MapOutline {
  map: string;
  bounds: { min: [number, number, number]; max: [number, number, number] };
  layers: MapLayer[];
}
