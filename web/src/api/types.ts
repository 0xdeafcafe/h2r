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
  hash?: string;
}

export interface GamePlayer {
  index: number;
  name: string;
  xuid?: Xuid | null;
  team: number;
  local?: boolean;
}

export interface FramePlayer {
  index: number;
  alive: boolean;
  pos: [number, number, number];
  yaw: number;
  vehicle?: string | null;
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
  console?: string;
  map: string;
  mode: "campaign" | "multiplayer" | string;
  variant?: string | null;
  team_play: boolean;
  score_limit?: number | null;
  time_limit_s?: number | null;
  players: GamePlayer[];
}
export interface PlayerJoined { player: GamePlayer }
export interface PlayerLeft { index: number }
export interface PlayerKilled {
  tick: number;
  victim: number;
  killer?: number | null;
  weapon?: string | null;
  kind: KillKind;
}
export interface ScoreChanged { tick: number; players: PlayerScore[]; teams?: TeamScore[] | null }
export interface GameEnded {
  tick: number;
  reason: "finished" | "quit" | "lost" | string;
  winner?: number | null;
  players?: PlayerScore[] | GamePlayer[] | null;
}

export type StreamMessage =
  | { kind: "event"; event: WireEvent }
  | { kind: "frame"; frame: Frame }
  | { kind: "caught_up"; position: number };

// REST shapes.
export interface ListResponse<T> { items: T[]; next: number | string | null }

export interface AccountSummary { xuid: Xuid; gamertag: string; created_at?: string; online?: boolean }

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

export interface MapLayer { z: [number, number]; polygons: [number, number][][] }
export interface MapOutline {
  map: string;
  bounds: { min: [number, number, number]; max: [number, number, number] };
  layers: MapLayer[];
}
