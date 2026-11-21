import { A } from "@solidjs/router";
import { For, Show, createMemo, createResource, createSignal, onCleanup } from "solid-js";
import { shortHash } from "../admin/content";
import {
  decideScreenshot, fetchScreenshotPreview, fetchScreenshotQueue,
  type PendingScreenshot, type ScreenshotDecision, type ScreenshotSession,
} from "../admin/screenshots";
import { adminState } from "../admin/state";
import { LIVE_ADMIN } from "../routes";
import { identity } from "../state/identity";
import { live } from "../state/live";
import { DeckPage, Panel } from "./Deck";
import "../styles/deck-screenshots.css";

export default function ScreenshotsPage() {
  const context = createMemo(() => {
    const me = identity.me(), wk = live.wellKnown();
    return adminState.isMeAdmin() === true && me && wk && me.server_id === wk.id
      ? { api: live.server().api, aud: wk.id, me }
      : null;
  });

  return (
    <DeckPage
      title="Screenshot review" back={LIVE_ADMIN}
      trail={[["dashboard", "/"], ["server", "/dash/server"], ["live admin", LIVE_ADMIN], ["screenshots"]]}
    >
      <Show when={context()} keyed fallback={
        <Panel title="Admin access">
          <p class="dk-note">
            <Show when={identity.me()} fallback={<><A href="/dash/signin">Sign in</A> with a server admin account.</>}>
              {adminState.isMeAdmin() === false ? "Only server admins can review screenshots." : "Checking admin access…"}
            </Show>
          </p>
        </Panel>
      }>
        {(session) => <ScreenshotQueue session={session} />}
      </Show>
    </DeckPage>
  );
}

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function ScreenshotQueue(props: { session: ScreenshotSession }) {
  const [cursor, setCursor] = createSignal<string>();
  const [notice, setNotice] = createSignal("");
  const [page, { mutate, refetch }] = createResource(
    () => ({ cursor: cursor() }),
    ({ cursor }) => fetchScreenshotQueue(props.session, cursor),
  );
  const ready = () => !page.loading && !page.error ? page() : undefined;

  function switchPage(next?: string) {
    setNotice("");
    if (next === cursor()) void refetch();
    else setCursor(next);
  }

  function reviewed(sha256: string, action: ScreenshotDecision) {
    mutate((current) => current && { ...current, items: current.items.filter((item) => item.sha256 !== sha256) });
    setNotice(action === "approve" ? "Screenshot approved." : "Screenshot removed.");
  }

  return (
    <Panel title="Pending screenshots">
      <div class="dk-toolbar">
        <button type="button" class="dk-pill" disabled={page.loading} onClick={() => switchPage()}>
          {cursor() ? "First page" : "Refresh"}
        </button>
        <Show when={ready()?.next}>{(next) =>
          <button type="button" class="dk-pill" onClick={() => switchPage(next())}>Next page</button>
        }</Show>
      </div>
      <p class="dk-note" role="status">{notice()}</p>
      <Show when={page.loading}><p class="dk-note">Loading screenshots…</p></Show>
      <Show when={page.error}>{(error) =>
        <p class="dk-state dk-state--bad" role="alert">Could not load screenshots. {message(error())}</p>
      }</Show>
      <Show when={ready()}>{(result) =>
        <Show when={result().items.length} fallback={<p class="dk-note">{cursor() ? "No screenshots left on this page." : "No screenshots waiting for review."}</p>}>
          <ul class="dk-screenshot-list">
            <For each={result().items}>{(item) => <ScreenshotRow item={item} session={props.session} reviewed={reviewed} />}</For>
          </ul>
        </Show>
      }</Show>
    </Panel>
  );
}

function ScreenshotRow(props: {
  item: PendingScreenshot;
  session: ScreenshotSession;
  reviewed: (sha256: string, action: ScreenshotDecision) => void;
}) {
  const [preview, setPreview] = createSignal<string>();
  const [loading, setLoading] = createSignal(false);
  const [saving, setSaving] = createSignal<ScreenshotDecision>();
  const [problem, setProblem] = createSignal("");
  const { sha256, uploader, reason } = props.item;
  const label = shortHash(sha256);
  const imageId = `screenshot-${sha256}`;
  const controller = new AbortController();

  onCleanup(() => {
    controller.abort();
    const url = preview();
    if (url) URL.revokeObjectURL(url);
  });

  async function togglePreview() {
    const url = preview();
    if (url) {
      setPreview(undefined);
      URL.revokeObjectURL(url);
      return;
    }
    setLoading(true);
    setProblem("");
    try {
      const blob = await fetchScreenshotPreview(props.session, sha256, controller.signal);
      if (!controller.signal.aborted) setPreview(URL.createObjectURL(blob));
    } catch (error) {
      if (!controller.signal.aborted) setProblem(`Could not load the preview. ${message(error)}`);
    } finally {
      if (!controller.signal.aborted) setLoading(false);
    }
  }

  async function decide(action: ScreenshotDecision) {
    if (saving()) return;
    if (action === "remove" && !window.confirm(`Remove screenshot ${label}?`)) return;
    setSaving(action);
    setProblem("");
    try {
      await decideScreenshot(props.session, props.item, action);
      if (!controller.signal.aborted) props.reviewed(sha256, action);
    } catch (error) {
      if (!controller.signal.aborted) setProblem(`Could not ${action} the screenshot. ${message(error)}`);
    } finally {
      if (!controller.signal.aborted) setSaving(undefined);
    }
  }

  return (
    <li>
      <p class="dk-note"><code title={sha256}>{label}</code> · Uploader <code>{uploader}</code></p>
      <p>{reason}</p>
      <div class="dk-toolbar">
        <button type="button" class="dk-pill" disabled={loading()} aria-expanded={!!preview()} aria-controls={imageId} onClick={() => void togglePreview()}>
          {loading() ? "Loading preview…" : preview() ? "Hide image" : "Show image"}
        </button>
        <button type="button" class="dk-pill" disabled={!!saving() || !preview()} aria-label={`Approve screenshot ${label}`} onClick={() => void decide("approve")}>
          {saving() === "approve" ? "Approving…" : "Approve"}
        </button>
        <button type="button" class="dk-pill" disabled={!!saving()} aria-label={`Remove screenshot ${label}`} onClick={() => void decide("remove")}>
          {saving() === "remove" ? "Removing…" : "Remove"}
        </button>
      </div>
      <div id={imageId} hidden={!preview()}>
        <Show when={preview()}>{(url) => <img src={url()} alt={`Screenshot ${label} awaiting review`} />}</Show>
      </div>
      <Show when={problem()}><p class="dk-state dk-state--bad" role="alert">{problem()}</p></Show>
    </li>
  );
}
