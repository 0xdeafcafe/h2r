import type { GameState } from "../state/reducer";
import type { MapFrame } from "../components/MapCanvas";
import type { RosterEntry } from "../map/render";
import { frameHub } from "../state/live";
import { playerColour, readPalette, type MapPalette } from "../map/palette";

let palette: MapPalette | null = null;
export function pal(): MapPalette {
  return (palette ??= readPalette());
}

export function colourFor(g: Pick<GameState, "team_play">, team: number, index: number): string {
  return playerColour(pal(), g.team_play, team, index);
}

export function rosterFor(g: GameState): Map<number, RosterEntry> {
  const m = new Map<number, RosterEntry>();
  for (const p of Object.values(g.players)) m.set(p.index, { name: p.name, colour: colourFor(g, p.team, p.index) });
  return m;
}

export const MARKER_TTL_MS = 6000;

/** A live game's map frame: interpolated players plus fading death marks. */
export function liveFrameSource(gameId: string, roster: () => Map<number, RosterEntry>) {
  return (now: number): MapFrame => {
    const buf = frameHub.get(gameId);
    const r = roster();
    return {
      players: buf.sample(now),
      actors: buf.sampleActors(now),
      vehicles: buf.sampleVehicles(now),
      hits: buf.sampleHits(now),
      bsp: buf.sampleBsp(now),
      machines: buf.sampleMachines(now),
      objects: buf.sampleObjects(now),
      markers: buf.liveMarkers(now, MARKER_TTL_MS).map((m) => ({
        x: m.x, y: m.y, z: m.z,
        alpha: 1 - (now - m.bornAt) / MARKER_TTL_MS,
        colour: r.get(m.index)?.colour ?? "#555",
      })),
      seen: buf.bounds,
    };
  };
}

export function teamName(team: number): string {
  return ["Red", "Blue", "Green", "Gold", "Purple", "Pink", "Teal", "Brown"][team] ?? `Team ${team + 1}`;
}

export { elapsedSeconds, gameSeconds } from "../state/length";
