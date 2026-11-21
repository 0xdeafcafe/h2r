// A game variant's page (docs/live/live-api.md, "Game variants"): its
// name, description, author and game type, the server's settings summary,
// the script it uses (opened read-only in the workbench), its
// `.h2variant` file, and Delete for its author. The pure half is
// ../lib/variants.ts.

import { For, Show, createResource, createSignal } from "solid-js";
import { A, useNavigate, useParams } from "@solidjs/router";
import { live } from "../state/live";
import { identity } from "../state/identity";
import { statsPath } from "../routes";
import { HOME_CRUMB } from "./modes";
import { Crumbs, Panel, Title, playerHref } from "./Layout";
import { shortHash } from "../admin/content";
import { longDate } from "./fmt";
import { bundleHref, deleteVariant, fetchVariant, notFound, scriptHref, variantFileName, type Variant } from "../lib/variants";
import "../styles/rules.css";

const CRUMB: [string, string] = ["Game Types", statsPath("/rules")];

function VariantDetail(props: { variant: Variant }) {
  const v = () => props.variant;
  const navigate = useNavigate();
  const [busy, setBusy] = createSignal(false);
  const [msg, setMsg] = createSignal<string | null>(null);
  const mine = () => identity.me()?.xuid === v().author.xuid;

  async function remove() {
    const me = identity.me(), wk = live.wellKnown();
    if (!me || !wk || !confirm(`Delete the game variant "${v().name}"? Its link stops working.`)) return;
    setBusy(true);
    setMsg(null);
    try {
      await deleteVariant(live.server().api, wk.id, me, v().id);
      navigate(`${statsPath("/rules")}?tab=types`);
    } catch (e) {
      setMsg(`couldn't delete it: ${(e as Error).message}`);
      setBusy(false);
    }
  }

  return (
    <div class="s-cols">
      <aside class="s-left">
        <Title>Game Variant</Title>
        <Panel title="Script" sub={v().script_hash ? "the rules it plays by" : "plays by the game's own rules"}>
          <Show when={v().script_hash} fallback={<p class="s-pad s-rules-muted">No script: a plain {v().game_type} variant.</p>}>
            {(hash) => (
              <dl class="s-facts s-pad">
                <div><dt>Name</dt><dd>{v().script_name ?? "unnamed"}</dd></div>
                <div><dt>Hash</dt><dd><code title={hash()}>{shortHash(hash())}</code></dd></div>
                <div><dt>[ <A href={scriptHref(hash())}>open in the workbench</A> ]</dt><dd /></div>
              </dl>
            )}
          </Show>
        </Panel>
      </aside>
      <div class="s-right">
        <Title class="s-title--cap">{v().name}</Title>
        <div class="s-matte s-variant">
          <Show when={v().description}><p class="s-variant-desc">{v().description}</p></Show>
          <dl class="s-facts">
            <div><dt>Author</dt><dd><A href={playerHref(v().author.gamertag)}>{v().author.gamertag || v().author.xuid}</A></dd></div>
            <div><dt>Game type</dt><dd>{v().game_type}</dd></div>
            <div><dt>Published</dt><dd>{longDate(v().created)}</dd></div>
            <Show when={v().visibility === "unlisted"}><div><dt>Visibility</dt><dd>Unlisted: only people with the link see it</dd></div></Show>
          </dl>
          <div class="s-toolbar s-variant-bar">
            <a class="s-btn" href={bundleHref(live.server().base, v().bundle_url)} download={variantFileName(v().name)}>Download .h2variant</a>
            <Show when={mine()}>
              <button class="s-btn" onClick={() => void remove()} disabled={busy()}>Delete</button>
            </Show>
          </div>
          <Show when={msg()}>{(m) => <p class="s-pad s-rules-bad">{m()}</p>}</Show>
          <Show when={v().settings.length} fallback={<p class="s-pad s-rules-muted">The server sent no settings.</p>}>
            <table class="s-grid">
              <thead><tr><th>Setting</th><th>Value</th></tr></thead>
              <tbody>
                <For each={v().settings}>{(s) => <tr><td>{s.label}</td><td>{s.value}</td></tr>}</For>
              </tbody>
            </table>
          </Show>
        </div>
      </div>
    </div>
  );
}

export default function VariantPage() {
  const params = useParams<{ id: string }>();
  const [variant] = createResource(
    () => ({ id: params.id ?? "", api: live.server().api }),
    ({ id, api }) => fetchVariant(api, id),
  );
  const loaded = () => (!variant.error && !variant.loading ? variant() ?? null : null);
  return (
    <>
      <Crumbs trail={[HOME_CRUMB, CRUMB, [loaded()?.name ?? "Game Variant"]]} />
      <Show when={!variant.error} fallback={
        <div class="s-full">
          <Title class="s-title--bar">Game Variant Not Found</Title>
          <p class="s-pad">{notFound(variant.error) ? "No game variant has that id. It may have been deleted." : "Couldn't load this game variant."} <A href={statsPath("/rules")}>Game Types</A></p>
        </div>
      }>
        <Show when={loaded()} keyed fallback={<p class="s-loading">Loading game variant…</p>}>
          {(v) => <VariantDetail variant={v} />}
        </Show>
      </Show>
    </>
  );
}
