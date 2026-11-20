import { Show, createSignal } from "solid-js";
import { CONTENT_WRITE, adminCall } from "../admin/content";
import { packPath, type ModPack } from "../lib/modPacks";
import { safeSourceUrl } from "../lib/maps";
import { identity } from "../state/identity";
import { live } from "../state/live";

function saveForm(changed: () => void) {
  const [busy, setBusy] = createSignal(false);
  const [message, setMessage] = createSignal("");
  async function submit(e: SubmitEvent, request: (data: FormData) => [string, RequestInit]) {
    e.preventDefault();
    const me = identity.me(), wk = live.wellKnown();
    if (!me || !wk || busy()) return;
    const form = e.currentTarget as HTMLFormElement;
    setBusy(true);
    setMessage("");
    try {
      const [path, init] = request(new FormData(form));
      await adminCall(live.server().api, path, wk.id, me, CONTENT_WRITE, init, fetch);
      setMessage("Saved.");
      changed();
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  }
  return { submit, busy, message };
}

export function PackMetadataForm(props: { pack: ModPack; changed: () => void }) {
  const { submit, busy, message } = saveForm(props.changed);
  const p = props.pack;
  return <form class="s-pack-form" onSubmit={(e) => void submit(e, (data) => {
    const field = (name: string) => String(data.get(name) ?? "").trim();
    const authors = field("authors").split(/\r?\n/).map((name) => name.trim()).filter(Boolean);
    if (!authors.length) throw new Error("Enter at least one author.");
    const source = field("source_url");
    if (source && !safeSourceUrl(source)) throw new Error("Use an http or https source URL.");
    return [packPath(p.id), { method: "PUT", headers: { "content-type": "application/json" },
      body: JSON.stringify({
        name: field("name"), authors, description: field("description") || undefined,
        source_url: source || undefined, version: field("version") || undefined,
        year: field("year") ? Number(field("year")) : undefined, advisory: field("advisory") || undefined,
        members: p.members.map((m) => m.map_id),
      }),
    }];
  })}>
    <label>Pack name <input name="name" maxLength={120} value={p.name} required /></label>
    <label>Made by (one author per line) <textarea name="authors" value={p.authors.join("\n")} required rows={2} /></label>
    <label>Description <textarea name="description" maxLength={4000} value={p.description ?? ""} rows={3} /></label>
    <label>Version <input name="version" value={p.version ?? ""} maxLength={24} /></label>
    <label>Release year <input name="year" type="number" min={1990} max={9999} value={p.year ?? ""} /></label>
    <label>Advisory <select name="advisory">
      <option value="" selected={!p.advisory}>None</option>
      <option value="cheating_pack" selected={p.advisory === "cheating_pack"}>Cheating pack</option>
      <option value="not_for_online" selected={p.advisory === "not_for_online"}>Not for online play</option>
    </select></label>
    <label>Source URL <input name="source_url" maxLength={300} type="url" value={p.source_url ?? ""} /></label>
    <button type="submit" class="s-btn" disabled={busy()}>Save pack</button>
    <Show when={message()}>{(msg) => <p role="status">{msg()}</p>}</Show>
  </form>;
}

export function ScreenshotCaptureHelp(props: { id: string }) {
  return <details class="s-pack-tools"><summary>Add in-game screenshots</summary>
    <p class="s-pack-form">Run <code>h2 live mods capture {props.id}</code> while signed in as the uploader. Screenshots may wait for review before appearing here.</p>
  </details>;
}
