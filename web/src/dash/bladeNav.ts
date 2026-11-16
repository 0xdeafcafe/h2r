// What up, down, left, right, A and X do on a column of blades (Blades.tsx),
// kept apart from the component so it can be tested without a DOM.

import type { Accessor, JSX } from "solid-js";
import type { Action } from "./input";

export interface Blade {
  key: string;
  label: JSX.Element;
  value?: JSX.Element;
  /** A: what choosing the row does. A row without one is a fact, skipped by up and down. */
  act?: () => void;
  /** X: the row's second action, and its prompt label. */
  alt?: { label: string; run: () => void };
  /** Left and right on the row (a cycling choice like "lasts 7 days"). */
  step?: (by: 1 | -1) => void;
  /** A text field in the row. Up and down still move on from it. */
  field?: { value: string; onInput: (v: string) => void; placeholder?: string; maxLength?: number; onEnter?: () => void };
  disabled?: boolean;
  /** Greyed (a used or expired invite, say). */
  old?: boolean;
  /** The lit "go" style: the row that does the plate's main thing. */
  go?: boolean;
}

export interface BladeControls {
  /** One step of keyboard or gamepad input; whether it meant anything here. */
  act(a: Action): boolean;
  /** The selected row's X action label, if it has one. */
  alt: Accessor<string | null>;
  sel: Accessor<number>;
}

/** Whether a row can be selected: not a plain fact, not greyed or disabled. */
export const isLive = (b: Blade | undefined): boolean => !!b && !b.disabled && !b.old && (!!b.act || !!b.field || !!b.step);

/** The next selectable row from `from`, going `dir`; `from` itself when there is none. */
export function nextBlade(blades: readonly Blade[], from: number, dir: 1 | -1): number {
  for (let i = from + dir; i >= 0 && i < blades.length; i += dir) if (isLive(blades[i])) return i;
  return from;
}

/** The first selectable row (0 when none is). */
export function firstBlade(blades: readonly Blade[]): number {
  const i = blades.findIndex(isLive);
  return i < 0 ? 0 : i;
}

/** What an input does on a column of blades, as data (tested in dash-blades.test.ts). */
export function bladeAction(blades: readonly Blade[], sel: number, a: Action):
  | { kind: "move"; to: number }
  | { kind: "act" | "alt" | "step"; by?: 1 | -1 }
  | null {
  const b = blades[sel];
  switch (a) {
    case "up": case "down": {
      const to = nextBlade(blades, sel, a === "down" ? 1 : -1);
      return to === sel ? null : { kind: "move", to };
    }
    case "left": case "right": return b?.step && !b.disabled ? { kind: "step", by: a === "right" ? 1 : -1 } : null;
    case "select": return b && !b.disabled && (b.act || b.field) ? { kind: "act" } : null;
    case "alt": return b?.alt && !b.disabled ? { kind: "alt" } : null;
    default: return null;
  }
}

