// The private engine event can carry host identity, XUID and bot parameters.
// A web lifecycle record contains just the public, displayable projection.
import type { FilmSeat } from "./host-film";

export type FilmLifecycle =
  | {kind: "join"; player: FilmSeat}
  | {kind: "leave" | "quit"; seat: number}
  | {kind: "team"; seat: number; team: number}
  | {kind: "host_migration"}
  | {kind: "pause"; paused: boolean};

export interface FilmLifecycleBatch {
  tick: number;
  closing: boolean;
  changes: FilmLifecycle[];
}

const utf8 = new TextDecoder("utf-8", {fatal: true});
function object(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === "object" && !Array.isArray(v);
}
function seat(v: unknown): v is number { return typeof v === "number" && Number.isInteger(v) && v >= 0 && v < 16; }
function team(v: unknown): v is number { return typeof v === "number" && Number.isInteger(v) && v >= -2147483648 && v <= 2147483647; }
function invalid(): never { throw new Error("Invalid host film lifecycle event"); }

export function decodeFilmLifecycle(payload: Uint8Array): FilmLifecycle[] {
  if (payload.length > (64 << 10)) invalid();
  const values: unknown = JSON.parse(utf8.decode(payload));
  if (!Array.isArray(values) || values.length > 256) invalid();
  return values.map((v: unknown): FilmLifecycle => {
    if (!object(v)) invalid();
    switch (v.kind) {
      case "join": {
        const p = v.player;
        if (!object(p) || !seat(p.seat) || !team(p.team) || typeof p.name !== "string" ||
          !(p.kind === "local" || p.kind === "remote" || p.kind === "phantom" || p.kind === "bot")) invalid();
        return {kind: "join", player: {seat: p.seat, team: p.team, name: p.name, kind: p.kind}};
      }
      case "leave": case "quit":
        return seat(v.seat) ? {kind: v.kind, seat: v.seat} : invalid();
      case "team":
        return seat(v.seat) && team(v.team) ? {kind: "team", seat: v.seat, team: v.team} : invalid();
      case "host_migration":
        return {kind: "host_migration"};
      case "pause":
        return typeof v.paused === "boolean" ? {kind: "pause", paused: v.paused} : invalid();
      default:
        return invalid();
    }
  });
}

export function lifecycleLabel(e: FilmLifecycle): string {
  switch (e.kind) {
    case "join": return e.player.name + " joined";
    case "leave": return `Player ${e.seat + 1} left`;
    case "quit": return `Player ${e.seat + 1} quit`;
    case "team": return `Player ${e.seat + 1} changed team`;
    case "host_migration": return "Host migration";
    case "pause": return e.paused ? "Paused" : "Resumed";
  }
}
