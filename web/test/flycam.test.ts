import { describe, expect, test } from "bun:test";
import { BOOST, MAX_PITCH, SPEED_MAX, SPEED_MIN, addMoves, anglesOf, approach, deadzone, keyBoost, keyMove, lookStep, moveIsIdle, padInput, speedLabel, speedStep } from "../src/viewer/flycam";

const keys = (...c: string[]) => new Set(c);
const pad = (axes: number[], pressed: number[] = [], values: Record<number, number> = {}) => ({
  axes,
  buttons: Array.from({ length: 17 }, (_, i) => ({ pressed: pressed.includes(i), value: values[i] ?? (pressed.includes(i) ? 1 : 0) })),
});

describe("keys to a move", () => {
  test("no keys is idle; WASD and arrows are the same", () => {
    expect(moveIsIdle(keyMove(keys()))).toBe(true);
    expect(keyMove(keys("KeyW"))).toEqual({ forward: 1, right: 0, up: 0 });
    expect(keyMove(keys("ArrowUp"))).toEqual(keyMove(keys("KeyW")));
    expect(keyMove(keys("KeyA")).right).toBe(-1);
    expect(keyMove(keys("ArrowRight")).right).toBe(1);
  });
  test("opposite keys cancel", () => {
    expect(moveIsIdle(keyMove(keys("KeyW", "KeyS")))).toBe(true);
    expect(keyMove(keys("KeyQ", "KeyE")).up).toBe(0);
  });
  test("Q, Ctrl go down; E, Space go up", () => {
    expect(keyMove(keys("KeyQ")).up).toBe(-1);
    expect(keyMove(keys("ControlLeft")).up).toBe(-1);
    expect(keyMove(keys("KeyE")).up).toBe(1);
    expect(keyMove(keys("Space")).up).toBe(1);
  });
  test("a diagonal is no faster than straight", () => {
    const m = keyMove(keys("KeyW", "KeyD", "Space"));
    expect(Math.hypot(m.forward, m.right, m.up)).toBeCloseTo(1, 6);
  });
  test("shift boosts", () => {
    expect(keyBoost(keys("ShiftLeft"))).toBe(true);
    expect(keyBoost(keys("KeyW"))).toBe(false);
    expect(BOOST).toBeGreaterThan(1);
  });
});

describe("looking", () => {
  test("dragging right turns the view right (yaw falls), dragging down looks down", () => {
    const r = lookStep(0, 0, 0.1, 0.2);
    expect(r.yaw).toBeCloseTo(-0.1, 9);
    expect(r.pitch).toBeCloseTo(-0.2, 9);
  });
  test("pitch is clamped short of straight up and down", () => {
    expect(lookStep(0, 1, 0, -100).pitch).toBe(MAX_PITCH);
    expect(lookStep(0, -1, 0, 100).pitch).toBe(-MAX_PITCH);
    expect(MAX_PITCH).toBeLessThan(Math.PI / 2);
  });
  test("yaw wraps", () => {
    const y = lookStep(3, 0, -1, 0).yaw;
    expect(Math.abs(y)).toBeLessThanOrEqual(Math.PI);
    expect(y).toBeCloseTo(4 - 2 * Math.PI, 9);
  });
  test("angles of a direction (-z is yaw 0)", () => {
    expect(anglesOf(0, 0, -1).yaw).toBeCloseTo(0, 9);
    expect(anglesOf(0, 0, -1).pitch).toBeCloseTo(0, 9);
    expect(anglesOf(-1, 0, 0).yaw).toBeCloseTo(Math.PI / 2, 9);
    expect(anglesOf(0, 1, 0).pitch).toBe(MAX_PITCH);
    expect(anglesOf(0, -1, -1).pitch).toBeCloseTo(-Math.PI / 4, 9);
  });
});

describe("speed", () => {
  test("wheel up is faster, down is slower, by steps", () => {
    expect(speedStep(1, -100)).toBeGreaterThan(1);
    expect(speedStep(1, 100)).toBeLessThan(1);
    expect(speedStep(speedStep(1, -100), 100)).toBeCloseTo(1, 9);
  });
  test("a big flick is a few notches, not a jump", () => {
    expect(speedStep(1, -10000)).toBeCloseTo(1.2 ** 3, 9);
  });
  test("clamped, and no wheel is no change", () => {
    expect(speedStep(SPEED_MAX, -100)).toBe(SPEED_MAX);
    expect(speedStep(SPEED_MIN, 100)).toBe(SPEED_MIN);
    expect(speedStep(2, 0)).toBe(2);
    expect(speedStep(2, NaN)).toBe(2);
  });
  test("the label", () => {
    expect(speedLabel(1)).toBe("Speed ×1.0");
    expect(speedLabel(0.25)).toBe("Speed ×0.25");
    expect(speedLabel(12)).toBe("Speed ×12");
  });
});

describe("easing", () => {
  test("no tau means no easing", () => {
    expect(approach(0, 5, 0.016, 0)).toBe(5);
  });
  test("the same time in different steps lands in the same place", () => {
    let a = 0;
    for (let i = 0; i < 60; i++) a = approach(a, 1, 1 / 60, 0.1);
    let b = 0;
    for (let i = 0; i < 30; i++) b = approach(b, 1, 1 / 30, 0.1);
    expect(a).toBeCloseTo(b, 6);
  });
  test("it settles exactly", () => {
    let v = 1;
    for (let i = 0; i < 200; i++) v = approach(v, 0, 0.05, 0.1);
    expect(v).toBe(0);
  });
});

describe("gamepad", () => {
  test("a stick inside the dead zone is nothing; the edge of it is not a jump", () => {
    expect(deadzone(0.1)).toBe(0);
    expect(deadzone(-0.15)).toBe(0);
    expect(deadzone(0.16)).toBeLessThan(0.02);
    expect(deadzone(1)).toBe(1);
    expect(deadzone(-1)).toBe(-1);
    expect(deadzone(NaN)).toBe(0);
  });
  test("no pad, a resting pad: idle", () => {
    expect(moveIsIdle(padInput(null).move)).toBe(true);
    const p = padInput(pad([0.05, -0.1, 0.02, 0.1]));
    expect(moveIsIdle(p.move)).toBe(true);
    expect(p.lookX).toBe(0);
    expect(p.boost).toBe(false);
  });
  test("left stick up is forward, right is right; right stick looks", () => {
    const p = padInput(pad([1, -1, -0.5, 0.5]));
    expect(p.move.right).toBeGreaterThan(0);
    expect(p.move.forward).toBeGreaterThan(0);
    expect(p.lookX).toBeLessThan(0);
    expect(p.lookY).toBeGreaterThan(0);
    expect(Math.hypot(p.move.right, p.move.forward)).toBeLessThanOrEqual(1.000001);
  });
  test("triggers and bumpers go up and down, A boosts", () => {
    expect(padInput(pad([0, 0, 0, 0], [5])).move.up).toBe(1);
    expect(padInput(pad([0, 0, 0, 0], [4])).move.up).toBe(-1);
    expect(padInput(pad([0, 0, 0, 0], [], { 7: 0.6 })).move.up).toBeGreaterThan(0);
    expect(padInput(pad([0, 0, 0, 0], [], { 6: 0.6 })).move.up).toBeLessThan(0);
    expect(padInput(pad([0, 0, 0, 0], [0])).boost).toBe(true);
  });
  test("pad and keys add, still no longer than 1", () => {
    const m = addMoves({ right: 1, up: 0, forward: 0 }, { right: 0, up: 0, forward: 1 });
    expect(Math.hypot(m.right, m.forward)).toBeCloseTo(1, 9);
  });
});
