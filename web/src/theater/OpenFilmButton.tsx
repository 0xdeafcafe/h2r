import { Show } from "solid-js";
import { openFilmHref } from "./open";

export function OpenFilmButton(props: { server: string; film: string; clip?: string; search: string }) {
  return (
    <Show when={openFilmHref(props.server, props.film, props.clip, props.search)}>
      {(href) => <a href={href()}>Open in Halo 2</a>}
    </Show>
  );
}
