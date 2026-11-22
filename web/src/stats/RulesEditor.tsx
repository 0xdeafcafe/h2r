// The workbench's script editor (docs/game/rules-engine.md, "Workbench"):
// Monaco with the `h2rules` language, loaded by a dynamic import on first
// mount so neither Monaco nor its worker is in the main bundle. The value
// is the page's; edits come back through onChange. Check errors and
// warnings show as markers, with their quick fixes (Ctrl/Cmd+.) and
// rename (F2) from the IDE route; the editor is read-only while the page says so, and Monaco's
// own layout follows the box's size. When Monaco can't load, a plain
// textarea stands in.

import { Show, createEffect, createSignal, onCleanup, onMount } from "solid-js";
import { offsetOf, type Problem } from "../lib/rules";
import type { Mounted } from "./rulesMonaco";

/** What the page may do to the editor. */
export interface RulesEditorApi {
  reveal(p: Problem): void;
  insert(snippet: string): void;
  focus(): void;
}

export default function RulesEditor(props: {
  value: string;
  onChange: (v: string) => void;
  readOnly: boolean;
  problems: readonly Problem[];
  /** The check's warnings, marked apart from its errors. */
  warnings?: readonly Problem[];
  /** The Live API base the IDE providers ask. */
  api: string;
  onReady?: (api: RulesEditorApi) => void;
}) {
  let host: HTMLDivElement | undefined;
  let area: HTMLTextAreaElement | undefined;
  let mod: typeof import("./rulesMonaco") | undefined;
  let gone = false;
  const [ed, setEd] = createSignal<Mounted | null>(null);
  const [failed, setFailed] = createSignal<string | null>(null);

  onMount(() => {
    void (async () => {
      try {
        mod = await import("./rulesMonaco");
        mod.setApi(props.api);
        await mod.setup();
        if (gone || !host) return;
        setEd(mod.mount(host, props.value, props.readOnly, (v) => props.onChange(v)));
      } catch (e) {
        if (!gone) setFailed((e as Error).message || "the editor didn't load");
      }
    })();
    props.onReady?.({
      reveal: (p) => {
        const e = ed();
        if (e) { e.reveal(p); return; }
        if (!area || p.line <= 0) return;
        const at = offsetOf(props.value, p.line, p.col);
        area.focus();
        area.setSelectionRange(at, Math.min(props.value.length, at + 1));
      },
      insert: (snippet) => {
        const e = ed();
        if (e) { e.insert(snippet); return; }
        if (!area || props.readOnly) return;
        const plain = snippet.replace(/\$\{\d+:([^}]*)\}/g, "$1").replace(/\$\d+/g, "").replace(/\t/g, "    ");
        const v = props.value, s = area.selectionStart, end = area.selectionEnd;
        props.onChange(v.slice(0, s) + plain + v.slice(end));
      },
      focus: () => { const e = ed(); if (e) e.focus(); else area?.focus(); },
    });
  });

  createEffect(() => { const e = ed(); if (e) e.setValue(props.value); });
  createEffect(() => { const e = ed(); if (e) e.setReadOnly(props.readOnly); });
  createEffect(() => { const e = ed(); if (e) e.setProblems(props.problems, props.warnings ?? []); });
  createEffect(() => { const api = props.api; mod?.setApi(api); });

  onCleanup(() => {
    gone = true;
    ed()?.dispose();
    setEd(null);
  });

  return (
    <div class="s-rules-editor">
      <Show when={!failed()} fallback={
        <textarea
          ref={area}
          class="s-rules-text"
          spellcheck={false}
          aria-label="Script source"
          readOnly={props.readOnly}
          value={props.value}
          onInput={(e) => props.onChange(e.currentTarget.value)}
        />
      }>
        {/* Monaco owns this box's children; the loading line is a sibling. */}
        <div class="s-rules-monaco" ref={host} />
        <Show when={!ed()}><p class="s-rules-loading">Loading the editor...</p></Show>
      </Show>
    </div>
  );
}
