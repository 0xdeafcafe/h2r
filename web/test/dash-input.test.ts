import { describe, expect, test } from "bun:test";
import {
  PAD_REPEAT_DELAY_MS, PAD_REPEAT_MS, POINTER_SLOP,
  inputModeAfter, keyAction, padActive, padHeld, padStep,
  type Action, type InputMode, type PadLike, type PadState,
} from "../src/dash/input";

/** A fake standard-mapping gamepad. */
function pad(pressed: number[] = [], axes: number[] = [0, 0, 0, 0]): PadLike {
  return { buttons: Array.from({ length: 17 }, (_, i) => ({ pressed: pressed.includes(i), value: pressed.includes(i) ? 1 : 0 })), axes };
}

describe("input mode", () => {
  test("each device takes over when it is used", () => {
    let m: InputMode = "keyboard";
    m = inputModeAfter(m, { kind: "pad" });
    expect(m).toBe("gamepad");
    m = inputModeAfter(m, { kind: "key", key: "ArrowDown" });
    expect(m).toBe("keyboard");
    m = inputModeAfter(m, { kind: "pointer", type: "down" });
    expect(m).toBe("pointer");
    m = inputModeAfter(m, { kind: "pad" });
    expect(m).toBe("gamepad");
    expect(inputModeAfter(m, { kind: "pointer", type: "wheel" })).toBe("pointer");
  });

  test("a bare modifier key changes nothing", () => {
    for (const key of ["Shift", "Control", "Alt", "Meta"]) expect(inputModeAfter("gamepad", { kind: "key", key })).toBe("gamepad");
    expect(inputModeAfter("pointer", { kind: "key", key: "Tab" })).toBe("keyboard");
  });

  test("a pointer that barely moves changes nothing; one that travels does", () => {
    expect(inputModeAfter("keyboard", { kind: "pointer", type: "move", moved: POINTER_SLOP - 1 })).toBe("keyboard");
    expect(inputModeAfter("gamepad", { kind: "pointer", type: "move" })).toBe("gamepad");
    expect(inputModeAfter("keyboard", { kind: "pointer", type: "move", moved: POINTER_SLOP })).toBe("pointer");
  });
});

describe("keys", () => {
  test("map onto actions", () => {
    const want: Array<[string, Action]> = [
      ["ArrowUp", "up"], ["ArrowDown", "down"], ["ArrowLeft", "left"], ["ArrowRight", "right"],
      ["Enter", "select"], ["a", "select"], ["A", "select"],
      ["Escape", "back"], ["Backspace", "back"], ["b", "back"], ["B", "back"],
      ["PageUp", "pageUp"], ["PageDown", "pageDown"],
    ];
    for (const [k, a] of want) expect(keyAction(k)).toBe(a);
    for (const k of ["x", " ", "Tab", "Shift", "F5"]) expect(keyAction(k)).toBeNull();
  });
});

describe("gamepad mapping", () => {
  test("buttons: a and start select, b backs out, shoulders page, d-pad steers", () => {
    const want: Array<[number, Action]> = [[0, "select"], [9, "select"], [1, "back"], [4, "pageUp"], [5, "pageDown"], [12, "up"], [13, "down"], [14, "left"], [15, "right"]];
    for (const [i, a] of want) expect([...padHeld(pad([i]))]).toEqual([a]);
    expect(padHeld(pad([2, 3, 6, 7, 8, 10, 11, 16])).size).toBe(0);
    expect(padHeld(pad()).size).toBe(0);
  });

  test("the left stick steers past its dead zone, along its stronger axis only", () => {
    expect([...padHeld(pad([], [0, -1]))]).toEqual(["up"]);
    expect([...padHeld(pad([], [0, 0.8]))]).toEqual(["down"]);
    expect([...padHeld(pad([], [-0.9, 0.2]))]).toEqual(["left"]);
    expect([...padHeld(pad([], [0.7, -0.6]))]).toEqual(["right"]);
    expect(padHeld(pad([], [0.3, -0.49])).size).toBe(0);
    // The right stick does nothing.
    expect(padHeld(pad([], [0, 0, 1, -1])).size).toBe(0);
  });

  test("a pad with too few buttons or axes is read without complaint", () => {
    expect(padHeld({ buttons: [{ pressed: true }], axes: [] })).toEqual(new Set<Action>(["select"]));
    expect(padHeld({ buttons: [], axes: [0.9] })).toEqual(new Set<Action>(["right"]));
  });

  test("padActive notices unmapped buttons and any stick", () => {
    expect(padActive(pad())).toBe(false);
    expect(padActive(pad([3]))).toBe(true);
    expect(padActive(pad([], [0, 0, 0.9, 0]))).toBe(true);
    expect(padActive(pad([], [0.2, 0.1, 0, 0]))).toBe(false);
  });
});

describe("gamepad polling", () => {
  const run = (frames: Array<[number, PadLike]>) => {
    let state: PadState = new Map();
    const fired: Array<[number, Action]> = [];
    for (const [t, p] of frames) {
      const r = padStep(p, state, t);
      state = r.state;
      for (const a of r.actions) fired.push([t, a]);
    }
    return fired;
  };

  test("a button fires once per press, however long it is held", () => {
    const frames: Array<[number, PadLike]> = [];
    for (let t = 0; t <= 2000; t += 16) frames.push([t, pad([0])]);
    frames.push([2016, pad()], [2032, pad([0])], [2048, pad([0])]);
    expect(run(frames)).toEqual([[0, "select"], [2032, "select"]]);
  });

  test("a held direction fires at once, repeats after a delay, then steadily", () => {
    const frames: Array<[number, PadLike]> = [];
    for (let t = 0; t <= 700; t += 10) frames.push([t, pad([13])]);
    const fired = run(frames);
    expect(fired.every(([, a]) => a === "down")).toBe(true);
    const times = fired.map(([t]) => t);
    expect(times[0]).toBe(0);
    expect(times[1]).toBe(PAD_REPEAT_DELAY_MS);
    expect(times[2]).toBe(PAD_REPEAT_DELAY_MS + PAD_REPEAT_MS);
    expect(times[3]).toBe(PAD_REPEAT_DELAY_MS + 2 * PAD_REPEAT_MS);
    expect(times.length).toBe(4);
  });

  test("letting go and pushing again starts over; the stick and the d-pad agree", () => {
    const fired = run([
      [0, pad([], [0, 1])], [100, pad([], [0, 1])], [116, pad([], [0, 0.1])], [132, pad([], [0, 1])],
      [148, pad([13], [0, 1])], [200, pad([], [0, -1])],
    ]);
    expect(fired).toEqual([[0, "down"], [132, "down"], [200, "up"]]);
  });

  test("a direction and a button held together each keep their own time", () => {
    const fired = run([[0, pad([12])], [50, pad([12, 0])], [PAD_REPEAT_DELAY_MS, pad([12, 0])]]);
    expect(fired).toEqual([[0, "up"], [50, "select"], [PAD_REPEAT_DELAY_MS, "up"]]);
  });
});
