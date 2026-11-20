import { watchLaunch } from "../lib/join";
import { createEffect, createSignal, onCleanup, Show } from "solid-js";

const install = "https://github.com/0xdeafcafe/h2r/blob/master/docs/live/live-custom-games.md#join-in-halo-2";

export function JoinLink(props: {
  href: string;
  class?: string;
  active?: boolean;
  select?: () => void;
  ref?: (link: HTMLAnchorElement) => void;
}) {
  const [hint, setHint] = createSignal(false);
  let cancel: (() => void) | undefined;
  function clicked() {
    props.select?.();
    cancel?.();
    setHint(false);
    cancel = watchLaunch(document, () => setHint(true));
  }
  createEffect(() => {
    props.href;
    setHint(false);
    onCleanup(() => cancel?.());
  });
  return (
    <span>
      <a href={props.href} rel="external" class={props.class} classList={{ on: props.active }}
        ref={(link) => props.ref?.(link)} onMouseEnter={props.select} onFocus={props.select}
        onClick={clicked}>Join in Halo 2</a>
      <Show when={hint()}>
        <small role="status">{" "}Need the game? <a href={install} rel="external">How to install</a>.</small>
      </Show>
    </span>
  );
}
