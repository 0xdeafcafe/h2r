// How the dashboard is being driven: keyboard, pointer (mouse or touch) or
// gamepad. One small store says which was used last, so every prompt on
// every screen agrees; and the keyboard and the gamepad both come down to
// the same few actions.

import { createSignal } from "solid-js";

export type InputMode = "keyboard" | "pointer" | "gamepad";
export type Action = "up" | "down" | "left" | "right" | "select" | "back" | "pageUp" | "pageDown";

// ---- which device was used last ----

export type InputEvent =
  | { kind: "key"; key: string }
  | { kind: "pointer"; type: "down" | "move" | "wheel"; moved?: number }
  | { kind: "pad" };

const MODIFIERS = new Set(["Shift", "Control", "Alt", "Meta", "CapsLock", "Dead", "Unidentified"]);
/** A pointer has to travel this far (px) before it counts: a mouse that is merely sitting on the page shouldn't take over. */
export const POINTER_SLOP = 6;

/** The mode after an input event. Bare modifier keys and a barely moving pointer change nothing. */
export function inputModeAfter(mode: InputMode, e: InputEvent): InputMode {
  switch (e.kind) {
    case "key": return MODIFIERS.has(e.key) ? mode : "keyboard";
    case "pointer": return e.type === "move" && (e.moved ?? 0) < POINTER_SLOP ? mode : "pointer";
    case "pad": return "gamepad";
  }
}

const coarse = () => typeof matchMedia !== "undefined" && matchMedia("(pointer: coarse)").matches;
const [mode, setMode] = createSignal<InputMode>(coarse() ? "pointer" : "keyboard");

/** The device used last. */
export const inputMode = mode;
export const noteInput = (e: InputEvent) => setMode(inputModeAfter(mode(), e));

/** Watch the keyboard and pointer for as long as the returned function isn't called. */
export function watchInput(): () => void {
  let at: [number, number] | null = null;
  const onKey = (e: KeyboardEvent) => noteInput({ kind: "key", key: e.key });
  const onDown = () => noteInput({ kind: "pointer", type: "down" });
  const onWheel = () => noteInput({ kind: "pointer", type: "wheel" });
  const onMove = (e: PointerEvent) => {
    if (!at) { at = [e.clientX, e.clientY]; return; }
    const moved = Math.hypot(e.clientX - at[0], e.clientY - at[1]);
    if (moved < POINTER_SLOP) return;
    at = [e.clientX, e.clientY];
    noteInput({ kind: "pointer", type: "move", moved });
  };
  window.addEventListener("keydown", onKey, true);
  window.addEventListener("pointerdown", onDown, true);
  window.addEventListener("pointermove", onMove, true);
  window.addEventListener("wheel", onWheel, { capture: true, passive: true });
  return () => {
    window.removeEventListener("keydown", onKey, true);
    window.removeEventListener("pointerdown", onDown, true);
    window.removeEventListener("pointermove", onMove, true);
    window.removeEventListener("wheel", onWheel, true);
  };
}

// ---- keyboard ----

/** The action a key stands for, if any. */
export function keyAction(key: string): Action | null {
  switch (key) {
    case "ArrowUp": return "up";
    case "ArrowDown": return "down";
    case "ArrowLeft": return "left";
    case "ArrowRight": return "right";
    case "PageUp": return "pageUp";
    case "PageDown": return "pageDown";
    case "Enter": case "a": case "A": return "select";
    case "Escape": case "Backspace": case "b": case "B": return "back";
    default: return null;
  }
}

// ---- gamepad ----

/** As much of a Gamepad as the mapping reads. */
export interface PadLike {
  buttons: ReadonlyArray<{ pressed: boolean; value?: number }>;
  axes: ReadonlyArray<number>;
}

/** The standard mapping's buttons: A and Start select, B backs out, the shoulders page, the d-pad steers. */
const PAD_BUTTONS: Array<[number, Action]> = [
  [0, "select"], [9, "select"], [1, "back"], [4, "pageUp"], [5, "pageDown"],
  [12, "up"], [13, "down"], [14, "left"], [15, "right"],
];
const STEER: ReadonlySet<Action> = new Set<Action>(["up", "down", "left", "right"]);

/** How far the stick must lean to count, and the pace of repeats while a direction is held. */
export const PAD_DEAD_ZONE = 0.5;
export const PAD_REPEAT_DELAY_MS = 380;
export const PAD_REPEAT_MS = 130;

/** What a pad is holding down right now. The d-pad and the left stick both steer; the stick only along its stronger axis. */
export function padHeld(pad: PadLike): Set<Action> {
  const held = new Set<Action>();
  for (const [i, a] of PAD_BUTTONS) if (pad.buttons[i]?.pressed) held.add(a);
  const x = pad.axes[0] ?? 0, y = pad.axes[1] ?? 0;
  if (Math.max(Math.abs(x), Math.abs(y)) >= PAD_DEAD_ZONE) {
    if (Math.abs(y) >= Math.abs(x)) held.add(y < 0 ? "up" : "down");
    else held.add(x < 0 ? "left" : "right");
  }
  return held;
}

/** When each held action next repeats (ms), or Infinity for buttons that only fire once. */
export type PadState = Map<Action, number>;

/** One poll: the actions to fire now. A press fires at once; a held direction repeats after a delay, then steadily; everything else fires once per press. */
export function padStep(pad: PadLike, state: PadState, now: number): { actions: Action[]; state: PadState } {
  const next: PadState = new Map();
  const actions: Action[] = [];
  for (const a of padHeld(pad)) {
    const due = state.get(a);
    if (due === undefined) {
      actions.push(a);
      next.set(a, STEER.has(a) ? now + PAD_REPEAT_DELAY_MS : Infinity);
    } else if (now >= due) {
      actions.push(a);
      next.set(a, now + PAD_REPEAT_MS);
    } else next.set(a, due);
  }
  return { actions, state: next };
}

/** Whether a pad is doing anything at all (to notice it before it presses something mapped). */
export function padActive(pad: PadLike): boolean {
  return pad.buttons.some((b) => b.pressed) || pad.axes.some((v) => Math.abs(v) >= PAD_DEAD_ZONE);
}

/** Drive `act` from any connected gamepad. Polls each frame, but only while a pad is connected. Returns the way to stop. */
export function watchGamepads(act: (a: Action) => void): () => void {
  if (typeof navigator === "undefined" || typeof navigator.getGamepads !== "function") return () => {};
  const states = new Map<number, PadState>();
  let raf = 0;
  const pads = () => Array.from(navigator.getGamepads()).filter((p): p is Gamepad => !!p && p.connected);
  const poll = (t: number) => {
    const list = pads();
    if (!list.length) { raf = 0; states.clear(); if (mode() === "gamepad") setMode("keyboard"); return; }
    for (const p of list) {
      const { actions, state } = padStep(p, states.get(p.index) ?? new Map(), t);
      states.set(p.index, state);
      if (actions.length || padActive(p)) noteInput({ kind: "pad" });
      for (const a of actions) act(a);
    }
    raf = requestAnimationFrame(poll);
  };
  const start = () => { if (!raf && pads().length) raf = requestAnimationFrame(poll); };
  window.addEventListener("gamepadconnected", start);
  window.addEventListener("gamepaddisconnected", start);
  start();
  return () => {
    window.removeEventListener("gamepadconnected", start);
    window.removeEventListener("gamepaddisconnected", start);
    cancelAnimationFrame(raf);
    raf = 0;
  };
}
