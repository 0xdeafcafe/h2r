// Wire types for the h2 live service, following docs/live/live-api.md.
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
  /** "stand" | "crouch" | "airborne" | "vehicle" | "dead". */
  stance?: string | null;
  /** Horizontal speed, game units a second. */
  speed?: number | null;
  /** One-shot actions since the previous frame, oldest first: "grenade", "melee", "reload", "jump", "switch", "pickup". */
  actions?: string[] | null;
  /** "frag" | "plasma". */
  grenade?: string | null;
}

/** A vehicle anywhere in the world. A player's or actor's `vehicle` is its `id`. */
export interface FrameVehicle {
  id: string;
  /** The vehicle tag's stem: "warthog", "ghost". */
  kind: string;
  pos: [number, number, number];
  /** Unit vectors, game coordinates (z up). */
  forward: [number, number, number];
  up: [number, number, number];
  /** Someone is in it; absent (older senders) means occupied. */
  occupied?: boolean;
}

/** Damage dealt: "p<index>" for players, else an object id. */
export interface FrameHit {
  from?: string | null;
  to?: string | null;
  weapon?: string | null;
  pos: [number, number, number];
  /** Body plus shield, as fractions of the victim's vitality. */
  amount?: number | null;
  /** "bullet", "explosion", "grenade", "melee", "fall", "collision", "vehicle", ... */
  kind?: string | null;
}

/** A door, elevator, the scarab, ...: map.glb's `machines` child `node`. */
export interface FrameMachine {
  node?: string | null;
  id: string;
  kind: string;
  pos: [number, number, number];
  forward: [number, number, number];
  up: [number, number, number];
  /** Device position 0..1. */
  position?: number | null;
}

/** An AI-controlled biped (campaign); only living ones are sent. */
export interface FrameActor {
  id: string;
  /** The biped tag's stem: "grunt", "marine", "elite". */
  species: string;
  pos: [number, number, number];
  yaw: number;
  aim?: [number, number, number] | null;
  weapon?: string | null;
  shots?: number | null;
  /** "covenant", "human", "flood", "sentinel", "prophet", "player", or a team index as text. */
  team?: string | null;
  vehicle?: string | null;
  stance?: string | null;
  speed?: number | null;
  actions?: string[] | null;
  grenade?: string | null;
}

export interface Frame {
  game_id: GameId;
  tick: number;
  players: FramePlayer[];
  vehicles?: FrameVehicle[];
  actors?: FrameActor[];
  hits?: FrameHit[];
  /** The structure BSP the game has loaded. */
  bsp?: number | null;
  machines?: FrameMachine[];
  /** Scenario placements not where the scenario put them (moved or gone),
   *  every one each frame, by kind then index. */
  objects?: FrameObject[];
}

/** A scenario placement that moved (`p`, `f`, `u`) or is gone: map.glb's
 *  `scenario` child `{kind: k, index: i}`. */
export interface FrameObject {
  /** The placement's kind, as map.glb's scenario extras name it: "bipd",
   *  "vehi", "weap", "eqip", "scen", "mach", "ctrl", "lifi", "ssce", "bloc",
   *  "crea", or "netgame" (a multiplayer spawn). */
  k: string;
  i: number;
  p?: [number, number, number];
  f?: [number, number, number];
  u?: [number, number, number];
  gone?: boolean;
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
export interface PresenceData { xuid: Xuid; state?: string; game_id?: GameId | null; /** What the player is doing, e.g. "Co-op: Outskirts". */ activity?: string | null }
/** What kind of game it was, all optional (older senders leave them out;
 *  `gameKind` infers the kind). */
export interface GameKindFields {
  /** `custom`, `matchmaking` or `campaign`. */
  kind?: string | null;
  /** Matchmaking only: the playlist the game came from. */
  playlist?: string | null;
  playlist_id?: string | null;
  /** Campaign only: `easy`, `normal`, `heroic` or `legendary`. */
  difficulty?: string | null;
  /** Campaign only: co-op partners on other machines. */
  online?: boolean | null;
  /** Skulls that were on: sorted unique indices 0..14 (lib/skulls.ts).
   *  Absent when none were, and for older games. */
  skulls?: number[] | null;
}
export interface GameStarted extends GameKindFields {
  game_id: GameId;
  /** The author who started the game, `acct:<xuid>@<server>` (written by the server). */
  host?: string;
  /** The console's own name for itself, if it gave one. */
  console?: string;
  map: string;
  /** The map package played (docs/game/maps.md §5); `map` is then its base map.
   *  Absent for retail maps. */
  map_id?: string;
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

export interface PresenceEntry { xuid: Xuid; gamertag: string; state: string; game_id?: GameId | null; activity?: string | null; since: string }

export interface GameSummary extends GameKindFields {
  game_id: GameId;
  /** The base map's name, also for a modded game. */
  map: string;
  /** The map package played; absent for retail maps. */
  map_id?: string;
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
