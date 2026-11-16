// The one row style every dashboard plate uses for things to do: a column
// of blades (focusable list rows) moved through with up and down, the
// selected one lit like a list row. A acts on it, X does its second action
// (revoke, say) where it has one. Rows may also be plain facts (no action)
// or hold a text field. The invites, accounts, import and server screens
// all build on this; the plate round them is parts.tsx's Plate.

import { Index, Match, Show, Switch, createEffect, createSignal } from "solid-js";
import { bladeAction, firstBlade, isLive as live, type Blade, type BladeControls } from "./bladeNav";
export type { Blade, BladeControls } from "./bladeNav";

/** A column of blades, with its controls handed to `bind` for the screen's input. */
export function Blades(props: { id: string; label: string; blades: Blade[]; bind?: (c: BladeControls) => void; class?: string }) {
  const [sel, setSel] = createSignal(firstBlade(props.blades));
  createEffect(() => {
    const n = props.blades.length;
    if (sel() >= n || !live(props.blades[sel()])) setSel(firstBlade(props.blades));
  });
  const rowId = (i: number) => `${props.id}-${i}`;
  const focus = (i: number) => queueMicrotask(() => document.getElementById(rowId(i))?.focus({ preventScroll: false }));
  function run(i: number) {
    const b = props.blades[i];
    if (!b || b.disabled) return;
    if (b.field) focus(i);
    else b.act?.();
  }
  props.bind?.({
    sel,
    alt: () => { const b = props.blades[sel()]; return b?.alt && !b.disabled ? b.alt.label : null; },
    act(a) {
      const r = bladeAction(props.blades, sel(), a);
      if (!r) return false;
      const b = props.blades[sel()];
      if (r.kind === "move") { setSel(r.to); focus(r.to); }
      else if (r.kind === "step") b?.step?.(r.by!);
      else if (r.kind === "alt") b?.alt?.run();
      else run(sel());
      return true;
    },
  });
  // <Index>, not <For>: the rows are rebuilt objects on every change, and a
  // keyed list would remake the row (and drop the caret) as a field is typed in.
  return (
    <ul class={`dx-blades ${props.class ?? ""}`} role="listbox" aria-label={props.label}>
      <Index each={props.blades}>
        {(b, i) => {
          const on = () => sel() === i && live(b());
          const common = { id: rowId(i), onMouseEnter: () => { if (live(b())) setSel(i); }, onFocus: () => { if (live(b())) setSel(i); } };
          const inner = () => <><span>{b().label}</span><Show when={b().value !== undefined}><b>{b().value}</b></Show></>;
          return (
            <li classList={{ on: on() }} role="option" aria-selected={on()}>
              <Switch>
                <Match when={b().field}>
                  <label class="dx-blade dx-blade--input" for={rowId(i)}>
                    <span>{b().label}</span>
                    <input
                      {...common} data-nav="rows" value={b().field!.value} maxLength={b().field!.maxLength} autocomplete="off" spellcheck={false}
                      placeholder={b().field!.placeholder} onInput={(e) => b().field!.onInput(e.currentTarget.value)}
                      onKeyDown={(e) => { const f = b().field; if (e.key === "Enter" && f?.onEnter) { e.preventDefault(); f.onEnter(); } }}
                    />
                  </label>
                </Match>
                <Match when={live(b()) || b().disabled}>
                  <button type="button" class="dx-blade" classList={{ "dx-blade--go": !!b().go }} {...common} disabled={b().disabled} onClick={() => { setSel(i); run(i); }}>
                    {inner()}
                  </button>
                </Match>
                <Match when={true}>
                  <div class="dx-blade dx-blade--fact" classList={{ "dx-blade--old": !!b().old }}>{inner()}</div>
                </Match>
              </Switch>
            </li>
          );
        }}
      </Index>
    </ul>
  );
}
