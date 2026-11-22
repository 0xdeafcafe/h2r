import { Index, Show, createEffect, createMemo, on, onCleanup } from "solid-js";

type ViewRow = {
  label: string;
  show?: boolean;
  disabled?: boolean;
} & (
  | { checked: boolean; pick: () => void }
  | { selected: boolean; pick: () => void }
  | { value?: string; panel: ViewPanel }
);

export interface ViewPanel { title: string; rows: readonly ViewRow[] }
export type { ViewRow };

export function ViewMenu(props: {
  panels: readonly ViewPanel[];
  picker?: boolean;
  open: (label: string) => void;
  back: () => void;
  close: (restoreFocus: boolean) => void;
  trigger: () => HTMLElement | undefined;
  backLabel?: string;
}) {
  let plate!: HTMLDivElement;
  let cursor = "";
  const panel = () => props.panels.at(-1)!;
  const isPicker = () => props.picker || props.panels.length > 1;
  const buttons = () => [...plate.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")];
  const location = createMemo(() => `${props.panels.length}:${panel().title}`);
  createEffect(on(location, requested => {
    queueMicrotask(() => {
      if (!plate.isConnected || location() !== requested) return;
      const items = buttons();
      const next = items.find(el => el.dataset.row === cursor)
        ?? items.find(el => el.getAttribute("aria-checked") === "true")
        ?? items.find(el => el.dataset.row)
        ?? items[0];
      next?.focus();
      cursor = "";
    });
  }));
  if (typeof document !== "undefined") {
    const outside = (e: PointerEvent) => {
      const target = e.target as Node;
      if (!plate.contains(target) && !props.trigger()?.contains(target)) props.close(false);
    };
    document.addEventListener("pointerdown", outside);
    onCleanup(() => document.removeEventListener("pointerdown", outside));
  }
  const key = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      closePanel();
      return;
    }
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp" && e.key !== "Home" && e.key !== "End") return;
    e.preventDefault();
    e.stopPropagation();
    const items = buttons();
    const i = items.indexOf(document.activeElement as HTMLButtonElement);
    const next = e.key === "Home" ? 0 : e.key === "End" ? items.length - 1
      : (i + (e.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
    items[next]?.focus();
  };
  const closePanel = () => { cursor = props.backLabel ?? ""; props.back(); };

  return (
    <div class="vo-plate vo-menu" ref={plate} role={isPicker() ? "dialog" : undefined}
      aria-label={isPicker() ? panel().title : undefined} onKeyDown={key}>
      <div>
        <Show when={isPicker()} fallback={<div class="vo-menu-title">{panel().title}</div>}>
          <button type="button" class="vo-menu-title" onClick={closePanel} aria-label={`Back from ${panel().title}`}>
            <span aria-hidden="true">‹ </span>{panel().title}
          </button>
        </Show>
        <div role="menu" aria-label={panel().title}>
          <Index each={panel().rows.filter(row => row.show !== false)}>{row => {
            const checked = () => {
              const r = row();
              return "checked" in r ? r.checked : "selected" in r ? r.selected : undefined;
            };
            const value = () => {
              const r = row();
              if ("checked" in r) return r.checked ? "On" : "Off";
              if ("selected" in r) return r.selected ? "✓" : "";
              return r.value ?? "›";
            };
            return (
              <button type="button" class="vo-row" data-row={row().label} disabled={row().disabled}
                role={"checked" in row() ? "menuitemcheckbox" : "selected" in row() ? "menuitemradio" : "menuitem"}
                aria-checked={checked()} aria-haspopup={"panel" in row() ? "dialog" : undefined}
                onClick={() => {
                  const r = row();
                  if ("panel" in r) props.open(r.label);
                  else {
                    r.pick();
                    if ("selected" in r) closePanel();
                  }
                }}>
                <span>{row().label}</span>
                <span class="vo-value">{value()}</span>
              </button>
            );
          }}</Index>
        </div>
      </div>
    </div>
  );
}
