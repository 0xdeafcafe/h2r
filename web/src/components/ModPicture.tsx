import { Show, createEffect, createSignal, onCleanup, type JSX } from "solid-js";
import { withAuthMark } from "../api/http";
import { issueToken } from "../admin/token";
import { identity } from "../state/identity";
import { live } from "../state/live";
import { mapTitle } from "../state/gameart";
import { mapPicture } from "./MapPicture";
import "../styles/mod-picture.css";

/** A signed-in reader can open a full-size private photo by the same local
 * URL used by the thumbnail. Release it when the reader or photo changes. */
export function ModPhoto(props: { src: string; alt: string; fullSize?: boolean; failed: () => void }) {
  const [local, setLocal] = createSignal<{ src: string; xuid: string; url: string }>();
  const source = () => {
    const me = identity.me();
    if (!me) return props.src;
    const current = local();
    return current?.src === props.src && current.xuid === me.xuid ? current.url : undefined;
  };
  createEffect(() => {
    const me = identity.me(), wk = live.wellKnown(), src = props.src, api = live.server().api;
    if (!me || !wk || !src.startsWith(api + "/")) return;
    const controller = new AbortController();
    let blobUrl: string | undefined;
    onCleanup(() => {
      controller.abort();
      if (blobUrl) URL.revokeObjectURL(blobUrl);
    });
    void (async () => {
      const token = await issueToken(me, wk.id, "read");
      const res = await fetch(withAuthMark(src), { signal: controller.signal, headers: { authorization: `Bearer ${token}` } });
      if (!res.ok) throw new Error(res.statusText);
      const blob = await res.blob();
      if (controller.signal.aborted) return;
      blobUrl = URL.createObjectURL(blob);
      setLocal({ src, xuid: me.xuid, url: blobUrl });
    })().catch(() => { if (!controller.signal.aborted) props.failed(); });
  });
  const picture = (url: string) => <img src={url} alt={props.alt} loading="lazy" decoding="async" onError={props.failed} />;
  return <Show when={source()}>{(url) => props.fullSize
    ? <a href={url()} target="_blank" rel="noopener noreferrer">{picture(url())}</a> : picture(url())}</Show>;
}

function StockPicture(props: { base?: string; name: string }) {
  const art = mapPicture(() => props.base);
  const [broken, setBroken] = createSignal("");
  return <Show when={art() && art() !== broken() ? art() : null} fallback={<span class="mod-picture-empty">No screenshot</span>}>
    {(url) => <>
      <img src={url()} alt={`Stock map picture of ${props.base ? mapTitle(props.base) : props.name}`} decoding="async" onError={() => setBroken(url())} />
      <span class="mod-picture-label">Stock map picture</span>
    </>}
  </Show>;
}

export default function ModPicture(props: { sources: string[]; base?: string; name: string }): JSX.Element {
  const [broken, setBroken] = createSignal<string[]>([]);
  const url = () => props.sources.find((src) => !broken().includes(src));
  return <span class="mod-picture"><Show when={url()} fallback={<StockPicture base={props.base} name={props.name} />}>
    {(src) => <ModPhoto src={src()} alt={props.name} failed={() => setBroken((b) => [...b, src()])} />}
  </Show></span>;
}
