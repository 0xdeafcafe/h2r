import { describe, expect, test } from "bun:test";
import type { Frame, FramePlayer, FrameProjectile } from "../src/api/types";
import { ActionWatch } from "../src/map/events";
import { FrameBuffer } from "../src/state/frames";
import { projectileColour, sampleProjectileTrails } from "../src/state/projectile-trails";
import { ReplayTimeline } from "../src/state/replay";
import { replayFrame } from "../src/viewer/session";

function frag(x: number, parent?: string, id = "a001000b"): FrameProjectile {
  return { id, kind: "frag_grenade", pos: [x, 2, 1], vel: [60, 0, 0], parent };
}

function frame(tick: number, projectiles?: FrameProjectile[]): Frame {
  return { game_id: "test-game", tick, players: [], projectiles };
}

describe("recorded projectile trails", () => {
  test("follows a frag's bounce using its observed type and leaves its throw animation alone", () => {
    const thrower: FramePlayer = { index: 0, alive: true, pos: [1, 2, 1], yaw: 0, aim: [1, 0, 0], grenade: "plasma" };
    const firing: FramePlayer = { index: 1, alive: true, pos: [2, 4, 1], yaw: 0, weapon: "battle_rifle" };
    const timeline = new ReplayTimeline([
      { ...frame(100, [frag(1)]), players: [{ ...thrower, actions: ["grenade"] }, { ...firing, fire: 1 }] },
      { ...frame(102, [frag(5)]), players: [thrower, firing] },
      { ...frame(104, [frag(3)]), players: [thrower, firing] },
      { ...frame(106), players: [{ ...thrower, actions: ["grenade"] }, firing] },
    ], []);
    const sample = (tick: number) => replayFrame(timeline, tick, new Map());
    const first = sample(100);
    expect(new ActionWatch().observe(first.players).map((a) => a.action)).toEqual(["grenade"]);
    expect(first.players[1]?.fireTicks).toEqual([100]);
    expect(first.projectileTrails).toEqual([{ id: "a001000b", kind: "frag_grenade", points: [[1, 2, 1]] }]);
    expect(sample(103).projectileTrails?.[0]?.points).toEqual([[1, 2, 1], [5, 2, 1], [4, 2, 1]]);
    expect(sample(104).projectileTrails?.[0]?.points).toEqual([[1, 2, 1], [5, 2, 1], [3, 2, 1]]);
    expect(sample(105).projectileTrails).toEqual(sample(104).projectileTrails);
    expect(sample(106).projectileTrails).toEqual([]);
    expect(projectileColour("frag_grenade")).toBe("#d8e86a");
    expect(projectileColour("plasma_grenade")).toBe("#6fd0ff");
  });

  test("resumes only from a fractional seek position, even within an ongoing throw", () => {
    const frames = [frame(100, [frag(1)]), frame(102, [frag(5)]), frame(104, [frag(3)])];
    expect(sampleProjectileTrails(frames, 103, 103)[0]?.points).toEqual([[4, 2, 1]]);
    expect(sampleProjectileTrails(frames, 104, 103)[0]?.points).toEqual([[4, 2, 1], [3, 2, 1]]);
  });

  test("starts another path on a cut, attachment change, disappearance or new salt", () => {
    const frames = [
      frame(100, [frag(1)]),
      { ...frame(102, [frag(5)]), cut: 102 },
      frame(104, [frag(8, "p4")]),
      frame(106),
      frame(108, [frag(9, "p4"), frag(10, undefined, "a002000b")]),
    ];
    expect(sampleProjectileTrails(frames, 101)[0]?.points).toEqual([[1, 2, 1]]);
    expect(sampleProjectileTrails(frames, 102)[0]?.points).toEqual([[5, 2, 1]]);
    expect(sampleProjectileTrails(frames, 103)[0]?.points).toEqual([[5, 2, 1]]);
    expect(sampleProjectileTrails(frames, 104)[0]?.points).toEqual([[8, 2, 1]]);
    expect(sampleProjectileTrails(frames, 106)).toEqual([]);
    expect(sampleProjectileTrails(frames, 108).map(({ id, points }) => [id, points])).toEqual([
      ["a001000b", [[9, 2, 1]]], ["a002000b", [[10, 2, 1]]],
    ]);
  });

  test("uses the live buffer's render clock and draws measured shots without any player actions", () => {
    const rocket = { ...frag(4), kind: "rocket" };
    const live = new FrameBuffer(48, 0);
    live.push(frame(100, [rocket]), 1000);
    const arrived = 1000 + 2000 / 30;
    live.push(frame(102, [{ ...rocket, pos: [6, 2, 1] }]), arrived);
    expect(sampleProjectileTrails(live.frames, live.renderTick(arrived + 1))[0])
      .toEqual({ id: rocket.id, kind: "rocket", points: [[4, 2, 1], [6, 2, 1]] });
    expect(projectileColour("rocket")).not.toBe(projectileColour("frag_grenade"));
    expect(projectileColour("unknown")).toBe("#e5dfd7");
  });

  test("keeps host film objects on the separate film path", () => {
    const filmed = { ...frame(100, [frag(1)]), film_world: { props: [], hill: null } };
    expect(sampleProjectileTrails([filmed], 100)).toEqual([]);
  });
});
