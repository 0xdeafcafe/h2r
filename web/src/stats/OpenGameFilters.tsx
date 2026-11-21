import { For, Show, createEffect, createSignal } from "solid-js";
import {
  SESSION_CAMPAIGNS, SESSION_CLANKERS, SESSION_DIFFICULTIES, SESSION_GAME_TYPES,
  SESSION_KINDS, SESSION_MAP_CATEGORIES, readSessionFilters, sessionVariant, type SessionFilters,
} from "../lib/sessionFilters";

function FilterSelect<T extends string | number | boolean>(props: {
  label: string;
  options: ReadonlyArray<readonly [T, string]>;
  value?: T;
  onPick: (value: T | undefined) => void;
}) {
  return (
    <label>
      {props.label}
      <select value={props.value === undefined ? "" : String(props.value)} onChange={(event) => {
        const selected = event.currentTarget.value;
        props.onPick(props.options.find(([value]) => String(value) === selected)?.[0]);
      }}>
        <option value="">Any</option>
        <For each={props.options}>{([value, label]) => <option value={String(value)}>{label}</option>}</For>
      </select>
    </label>
  );
}

export default function OpenGameFilters(props: {
  value: SessionFilters;
  variants: string[];
  apply: (value: SessionFilters) => void;
}) {
  const [open, setOpen] = createSignal(false);
  const [draft, setDraft] = createSignal(props.value);
  let toggle: HTMLButtonElement | undefined;
  createEffect(() => setDraft(props.value));

  const active = () => Object.values(props.value).filter((value) => value !== undefined).length;
  const invalidVariant = () => !!draft().variant?.trim() && sessionVariant(draft().variant) === undefined;
  function set<K extends keyof SessionFilters>(key: K, value: SessionFilters[K]) {
    setDraft((previous) => ({ ...previous, [key]: value }));
  }

  return (
    <>
      <header class="s-open-head">
        <h3 id="s-open-h">Open Games</h3>
        <button type="button" class="s-btn" aria-expanded={open()} aria-controls="s-open-filters"
          ref={(element) => { toggle = element; }} onClick={() => setOpen(!open())}>
          Filters<Show when={active()}>{(n) => ` (${n()})`}</Show>
        </button>
      </header>
      <form id="s-open-filters" class="s-open-filters" hidden={!open()} aria-label="Open game filters"
        onSubmit={(event) => {
          event.preventDefault();
          if (invalidVariant()) return;
          const filters = readSessionFilters(draft());
          setDraft(filters);
          props.apply(filters);
          setOpen(false);
          toggle?.focus();
        }}>
        <FilterSelect label="Type" options={SESSION_KINDS} value={draft().kind} onPick={(v) => set("kind", v)} />
        <FilterSelect label="Clankers" options={SESSION_CLANKERS} value={draft().clankers} onPick={(v) => set("clankers", v)} />
        <FilterSelect label="Game type" options={SESSION_GAME_TYPES} value={draft().game_type} onPick={(v) => set("game_type", v)} />
        <label>
          Variant
          <input type="text" list="s-open-variants" value={draft().variant ?? ""}
            placeholder="Any variant" maxLength={256} aria-invalid={invalidVariant()}
            aria-describedby={invalidVariant() ? "s-open-variant-error" : undefined}
            onInput={(event) => set("variant", event.currentTarget.value)} />
        </label>
        <datalist id="s-open-variants">
          <For each={props.variants}>{(name) => <option value={name} />}</For>
        </datalist>
        <FilterSelect label="Map category" options={SESSION_MAP_CATEGORIES} value={draft().map_category} onPick={(v) => set("map_category", v)} />
        <FilterSelect label="Campaign" options={SESSION_CAMPAIGNS} value={draft().campaign} onPick={(v) => set("campaign", v)} />
        <FilterSelect label="Difficulty" options={SESSION_DIFFICULTIES} value={draft().difficulty} onPick={(v) => set("difficulty", v)} />
        <label class="s-open-space">
          <input type="checkbox" checked={draft().has_space ?? false}
            onChange={(event) => set("has_space", event.currentTarget.checked || undefined)} />
          Has space
        </label>
        <Show when={invalidVariant()}>
          <p id="s-open-variant-error" class="s-open-variant-error" role="alert">Variant name is too long or contains an invalid character.</p>
        </Show>
        <div class="s-open-filter-actions">
          <button type="submit" class="s-btn" disabled={invalidVariant()}>Apply</button>
          <button type="button" class="s-btn" onClick={() => { setDraft({}); props.apply({}); }}>Clear filters</button>
        </div>
      </form>
    </>
  );
}
