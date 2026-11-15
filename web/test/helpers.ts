import type { WireEvent } from "../src/api/types";

let pos = 0;
export function resetPos() { pos = 0; }
export function ev(type: string, stream: string, data: unknown, at = "2026-11-15T20:00:00.000Z"): WireEvent {
  pos += 1;
  return { position: pos, stream, version: pos, type, at, data };
}
