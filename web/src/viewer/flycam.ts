// The flycam's maths, apart from three.js and the DOM so it can be tested:
// keys and sticks to a move, look deltas to yaw and pitch, the speed steps.

/** A move in the camera's own frame, each part -1..1: `right`, `up` (world
 *  up), `forward` (the way the camera looks, pitch included). */
export interface FlyMove { right: number; up: number; forward: number }

/** What a gamepad gives, one frame (the parts of the Gamepad API used). */
export interface PadState {
  axes: readonly number[];
  buttons: readonly { pressed: boolean; value: number }[];
}

export interface PadInput {
  move: FlyMove;
  /** Stick look, -1..1 each way (x right, y down). */
  lookX: number;
  lookY: number;
  boost: boolean;
}

export const NO_MOVE: FlyMove = { right: 0, up: 0, forward: 0 };

/** The `KeyboardEvent.code`s the flycam answers to. */
export const FLY_KEYS: ReadonlySet<string> = new Set([
  "KeyW", "KeyA", "KeyS", "KeyD", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight",
  "KeyQ", "KeyE", "Space", "ControlLeft", "ControlRight", "ShiftLeft", "ShiftRight",
]);

/** The pitch limit: just short of straight up or down, so the view never flips. */
export const MAX_PITCH = Math.PI / 2 - 0.02;
export const PAD_DEADZONE = 0.15;
export const SPEED_MIN = 0.05;
export const SPEED_MAX = 40;
/** Held with Shift (or the boost button). */
export const BOOST = 3;
/** How much one wheel notch changes the speed. */
export const SPEED_STEP = 1.2;

/** The move the held keys ask for: opposite keys cancel, and a diagonal is no faster. */
export function keyMove(keys: ReadonlySet<string>): FlyMove {
  const has = (...c: string[]) => c.some((k) => keys.has(k));
  const axis = (pos: boolean, neg: boolean) => (pos ? 1 : 0) - (neg ? 1 : 0);
  return limit({
    forward: axis(has("KeyW", "ArrowUp"), has("KeyS", "ArrowDown")),
    right: axis(has("KeyD", "ArrowRight"), has("KeyA", "ArrowLeft")),
    up: axis(has("KeyE", "Space"), has("KeyQ", "ControlLeft", "ControlRight")),
  });
}

export function keyBoost(keys: ReadonlySet<string>): boolean {
  return keys.has("ShiftLeft") || keys.has("ShiftRight");
}

/** A move no longer than 1. */
export function limit(m: FlyMove): FlyMove {
  const l = Math.hypot(m.right, m.up, m.forward);
  return l > 1 ? { right: m.right / l, up: m.up / l, forward: m.forward / l } : m;
}

/** Adds two moves, then limits the sum. */
export function addMoves(a: FlyMove, b: FlyMove): FlyMove {
  return limit({ right: a.right + b.right, up: a.up + b.up, forward: a.forward + b.forward });
}

/** A stick axis with its dead zone cut out, and what is left stretched back
 *  to 0..1 so there is no jump at the edge. */
export function deadzone(v: number, zone = PAD_DEADZONE): number {
  if (!Number.isFinite(v)) return 0;
  const a = Math.abs(v);
  if (a <= zone) return 0;
  return Math.sign(v) * Math.min(1, (a - zone) / (1 - zone));
}

/** A gamepad's ask: left stick moves (up is forward), right stick looks,
 *  (squared, see `stickCurve`), the right trigger or bumper goes up, the
 *  left goes down, a (button 0) or a stick click boosts. Layout is the standard mapping. */
export function padInput(pad: PadState | null | undefined): PadInput {
  if (!pad) return { move: NO_MOVE, lookX: 0, lookY: 0, boost: false };
  const b = (i: number) => pad.buttons[i]?.pressed || (pad.buttons[i]?.value ?? 0) > 0.5;
  const trig = (i: number) => deadzone(pad.buttons[i]?.value ?? 0, 0.1);
  const up = Math.max(trig(7), b(5) ? 1 : 0), down = Math.max(trig(6), b(4) ? 1 : 0);
  return {
    move: limit({ right: deadzone(pad.axes[0] ?? 0), forward: -deadzone(pad.axes[1] ?? 0), up: up - down }),
    lookX: stickCurve(deadzone(pad.axes[2] ?? 0)),
    lookY: stickCurve(deadzone(pad.axes[3] ?? 0)),
    boost: b(0) || b(10),
  };
}

export function moveIsIdle(m: FlyMove): boolean {
  return m.right === 0 && m.up === 0 && m.forward === 0;
}

/** Yaw and pitch after a look of (dx, dy) radians, dx right and dy down
 *  as on screen. Dragging right turns the view right, so yaw falls (it
 *  grows counter-clockwise); pitch is clamped. */
export function lookStep(yaw: number, pitch: number, dx: number, dy: number): { yaw: number; pitch: number } {
  return { yaw: wrapAngle(yaw - dx), pitch: Math.max(-MAX_PITCH, Math.min(MAX_PITCH, pitch - dy)) };
}

export function wrapAngle(a: number): number {
  const t = Math.PI * 2;
  return a - t * Math.round(a / t);
}

/** Yaw and pitch of a view direction (three.js axes: -z forward, y up). */
export function anglesOf(x: number, y: number, z: number): { yaw: number; pitch: number } {
  const l = Math.hypot(x, y, z) || 1;
  return { yaw: Math.atan2(-x, -z), pitch: Math.max(-MAX_PITCH, Math.min(MAX_PITCH, Math.asin(y / l))) };
}

/** Pixels of wheel movement that make one notch (a mouse wheel's click). */
export const WHEEL_NOTCH = 100;

/** A wheel event's movement in pixels, whatever unit it came in
 *  (`WheelEvent.deltaMode`: 0 pixels, 1 lines, 2 pages). Firefox scrolls
 *  three lines a notch; a page is taken as three notches. */
export function wheelPixels(deltaY: number, deltaMode = 0): number {
  if (!Number.isFinite(deltaY)) return 0;
  return deltaY * (deltaMode === 1 ? WHEEL_NOTCH / 3 : deltaMode === 2 ? WHEEL_NOTCH * 3 : 1);
}

/** The speed multiplier after wheel movement (positive deltaY, scrolling
 *  down, is slower). Proportional to the movement, so a trackpad's stream
 *  of small deltas changes the speed smoothly and adds up to what a
 *  mouse's notches would; one event moves at most three notches, so a
 *  flick is not a jump. Clamped. */
export function speedStep(speed: number, deltaY: number, deltaMode = 0): number {
  const px = wheelPixels(deltaY, deltaMode);
  if (!px) return speed;
  const notches = Math.max(-3, Math.min(3, px / WHEEL_NOTCH));
  const next = speed * SPEED_STEP ** -notches;
  return Math.max(SPEED_MIN, Math.min(SPEED_MAX, next));
}

/** Radians of look per pixel of mouse movement. */
export const MOUSE_LOOK = 0.0022;
/** Radians of look per pixel of a finger's drag (a finger moves further). */
export const TOUCH_LOOK = 0.004;
/** Radians a second at full stick. */
export const PAD_LOOK = 2.4;
/** One pointer event moving further than this (pixels) is a browser's
 *  pointer-lock glitch (Chrome reports a jump when the cursor is warped),
 *  not a hand: it is dropped. */
export const LOOK_SPIKE = 250;

/** A pointer movement to a look in radians (x right, y down), nothing for
 *  a spike. */
export function pointerLook(dx: number, dy: number, perPixel = MOUSE_LOOK): { dx: number; dy: number } {
  if (!Number.isFinite(dx) || !Number.isFinite(dy) || Math.abs(dx) > LOOK_SPIKE || Math.abs(dy) > LOOK_SPIKE) return { dx: 0, dy: 0 };
  return { dx: dx * perPixel, dy: dy * perPixel };
}

/** A stick's look response: squared (sign kept), so small tilts aim finely
 *  and full tilt still turns at full rate. */
export function stickCurve(v: number): number {
  return Math.sign(v) * v * v;
}

/** Which mouse buttons drag to look: the left (a click without a drag
 *  takes the pointer) and the right (held while flying, as in an editor). */
export function looksWith(button: number): boolean {
  return button === 0 || button === 2;
}

/** Two fingers from `a0`, `b0` to `a1`, `b1` (client pixels) as a move in
 *  the camera's frame, in pixels: spreading them flies forward, pinching
 *  back; dragging both moves the view with them as if the world were
 *  pushed (right drags the camera left, up drags it down). */
export function twoFingerMove(
  a0: readonly [number, number], b0: readonly [number, number],
  a1: readonly [number, number], b1: readonly [number, number],
): FlyMove {
  const spread = Math.hypot(a1[0] - b1[0], a1[1] - b1[1]) - Math.hypot(a0[0] - b0[0], a0[1] - b0[1]);
  const mx = (a1[0] + b1[0] - a0[0] - b0[0]) / 2, my = (a1[1] + b1[1] - a0[1] - b0[1]) / 2;
  const ok = [spread, mx, my].every(Number.isFinite);
  return ok ? { right: -mx, up: my, forward: spread } : NO_MOVE;
}

/** How the speed is told to the viewer. */
export function speedLabel(speed: number): string {
  return `Speed ×${speed >= 10 ? speed.toFixed(0) : speed.toFixed(speed >= 1 ? 1 : 2)}`;
}

/** Eases a velocity part toward what is wanted, frame-rate independent:
 *  after `tau` seconds it has covered 63% of the way. No `tau`, no easing
 *  (reduced motion). */
export function approach(v: number, want: number, dt: number, tau: number): number {
  if (tau <= 0 || dt <= 0) return tau <= 0 ? want : v;
  const k = 1 - Math.exp(-dt / tau);
  const next = v + (want - v) * k;
  return Math.abs(next - want) < 1e-4 ? want : next;
}
