import { Show, createSignal } from "solid-js";
import { A } from "@solidjs/router";
import { createClip, type Clip, type ClipInput } from "../api/films";
import { clock } from "../lib/format";
import { clipShareUrl, theaterHref } from "../routes";
import { TICKS_PER_SECOND } from "../state/frames";
import { identity } from "../state/identity";
import { live } from "../state/live";
import type { TickRange } from "../state/replay";
import { clipRangeError, type ClipMarks } from "./clipRange";

export default function ClipEditor(props: {
  game: string;
  bounds: TickRange;
  tick: () => number;
  marks: ClipMarks;
  setMarks: (m: ClipMarks) => void;
  view: () => Pick<ClipInput, "pov" | "camera">;
  close: () => void;
}) {
  const [title, setTitle] = createSignal("");
  const [pending, setPending] = createSignal(false);
  const [error, setError] = createSignal("");
  const [created, setCreated] = createSignal<Clip>();
  const [copied, setCopied] = createSignal("");
  const problem = () => clipRangeError(props.marks, props.bounds);
  const name = () => title().trim().replace(/\s+/g, " ");
  const titleTooLong = () => Array.from(name()).length > 60;
  const duration = () => ((props.marks.to_tick ?? 0) - (props.marks.from_tick ?? 0)) / TICKS_PER_SECOND;
  const time = (tick: number | null) => tick === null ? "—" : clock((tick - props.bounds.from_tick) / TICKS_PER_SECOND);
  const setMark = (key: keyof ClipMarks) => {
    setError("");
    props.setMarks({ ...props.marks, [key]: Math.round(props.tick()) });
  };
  async function submit(e: SubmitEvent) {
    e.preventDefault();
    const me = identity.me(), wk = live.wellKnown();
    const { from_tick, to_tick } = props.marks;
    if (pending() || !me || !wk || problem() || from_tick === null || to_tick === null || !name() || titleTooLong()) return;
    setPending(true);
    setError("");
    try {
      const clip = await createClip({ api: live.server().api, aud: wk.id, me }, props.game, {
        from_tick, to_tick, title: name(), ...props.view(),
      });
      setCreated(clip);
      props.setMarks({ from_tick: null, to_tick: null });
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setPending(false);
    }
  }
  async function copy(url: string) {
    try {
      await navigator.clipboard.writeText(url);
      setCopied("Link copied.");
    } catch {
      setCopied("Select and copy the link below.");
    }
  }
  return (
    <section class="theater-clip-editor" aria-label="Create a clip">
      <Show when={created()} fallback={
        <form onSubmit={(e) => void submit(e)}>
          <div class="theater-clip-marks">
            <button type="button" class="s-btn" disabled={pending()} onClick={() => setMark("from_tick")}>Mark in</button>
            <span>In {time(props.marks.from_tick)}</span>
            <button type="button" class="s-btn" disabled={pending()} onClick={() => setMark("to_tick")}>Mark out</button>
            <span>Out {time(props.marks.to_tick)}</span>
          </div>
          <p class="theater-clip-note" role="status">{problem() ?? `${clock(duration())} clip`}</p>
          <div class="theater-clip-fields">
            <label>Title <input required value={title()} onInput={(e) => setTitle(e.currentTarget.value)} disabled={pending()} aria-invalid={titleTooLong()} /></label>
            <button type="submit" class="s-btn" disabled={!live.wellKnown() || !!problem() || !name() || titleTooLong() || pending()}>{pending() ? "Creating…" : "Create clip"}</button>
            <button type="button" class="s-btn" disabled={pending()} onClick={props.close}>Cancel</button>
          </div>
          <Show when={titleTooLong()}><p class="s-bad" role="status">The title can be up to 60 characters.</p></Show>
          <Show when={error()}><p class="s-bad" role="alert">Couldn't create the clip. {error()}</p></Show>
        </form>
      }>
        {(clip) => {
          const share = () => clipShareUrl(live.server().base, clip().game, clip().id);
          return (
            <>
              <div class="theater-clip-fields">
                <label>Share link <input type="url" readonly value={share()} onFocus={(e) => e.currentTarget.select()} /></label>
                <button type="button" class="s-btn" onClick={() => void copy(share())}>Copy link</button>
                <A href={theaterHref(clip().game, clip().id)}>Watch clip</A>
                <button type="button" class="s-btn" onClick={props.close}>Done</button>
              </div>
              <Show when={copied()}><p role="status">{copied()}</p></Show>
            </>
          );
        }}
      </Show>
    </section>
  );
}
