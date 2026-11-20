// The moderation row acts on the pack. Individual map actions stay in its details.
import { For, Show, createMemo } from "solid-js";
import ModPicture from "../components/ModPicture";
import { formatBytes } from "../lib/maps";
import { memberName, packAdvisoryText, packCoverSources } from "../lib/modPacks";
import { modMakers } from "../lib/mods";
import { mapTitle } from "../state/gameart";
import {
  actionsFor, shortHash, type ContentList, type ContentState, type ContentTarget, type ModerateAction,
} from "./content";

type OnAction = (target: ContentTarget, id: string, action: ModerateAction) => void;

export function ModerationButtons(p: { target: ContentTarget; id: string; state: ContentState; busy: boolean; onAction: OnAction }) {
  return (
    <span class="adm-rec-btns adm-mod-btns">
      <For each={actionsFor(p.state)}>{(action) => (
        <button type="button" class="s-btn" disabled={p.busy} onClick={() => p.onAction(p.target, p.id, action)}>{action}</button>
      )}</For>
    </span>
  );
}

export default function ModPackTable(p: {
  content: ContentList;
  api: string;
  expanded: string | null;
  onExpand: (id: string | null) => void;
  busy: boolean;
  onAction: OnAction;
}) {
  const maps = createMemo(() => new Map(p.content.maps.map((m) => [m.map_id, m])));
  const packMembers = createMemo(() => new Set(p.content.packs.flatMap((pack) => pack.members.map((m) => m.map_id))));
  // An older server has only the map actions. Keep those rows actionable there.
  const standalone = () => p.content.maps.filter((m) => !packMembers().has(m.map_id));
  return (
    <div class="adm-pack-scroll">
      <table class="adm-records adm-mod adm-packs">
        <thead><tr><th scope="col" class="adm-grow">Name</th><th scope="col">Made by</th><th scope="col">Maps</th><th scope="col">Total size</th><th scope="col">State</th><th scope="col">Reports</th><th scope="col">Actions</th></tr></thead>
        <For each={p.content.packs}>{(pack) => {
          const open = () => p.expanded === pack.id;
          return (
            <tbody>
              <tr classList={{ off: pack.state !== "ok" }}>
                <th scope="row">
                  <button type="button" class="adm-pack-toggle" aria-expanded={open()} onClick={() => p.onExpand(open() ? null : pack.id)}>
                    <span aria-hidden="true" class="adm-pack-caret">{open() ? "▾" : "▸"}</span>
                    <span class="adm-pack-cover" aria-hidden="true">
                      <ModPicture sources={packCoverSources(p.api, pack)} base={pack.members[0]?.base.map} name={pack.name} />
                    </span>
                    <span>{pack.name}</span>
                  </button>
                </th>
                <td>{pack.made_by || "Uncredited"}</td><td>{pack.members.length}</td><td>{formatBytes(pack.size)}</td>
                <td class={`adm-state adm-state--${pack.state}`}>{pack.state}</td><td>{pack.reports}</td>
                <td><ModerationButtons target="pack" id={pack.id} state={pack.state} busy={p.busy} onAction={p.onAction} /></td>
              </tr>
              <Show when={open()}>
                <tr><td colSpan={7} class="adm-pack-details">
                  <p class="adm-pack-meta">Uploaded by {pack.uploader.gamertag}<Show when={pack.version}>{(version) => <> · Version {version()}</>}</Show>
                    <Show when={pack.year}>{(year) => <> · Released {year()}</>}</Show>
                    <Show when={packAdvisoryText(pack.advisory)}>{(advisory) => <> · {advisory()}</>}</Show>
                    <Show when={pack.source_url}>{(url) => <> · <a href={url()} target="_blank" rel="noreferrer">Source</a></>}</Show>
                  </p>
                  <Show when={pack.description}>{(text) => <p>{text()}</p>}</Show>
                  <table class="adm-records" aria-label={`Maps in ${pack.name}`}>
                    <thead><tr><th>Member</th><th>Base map</th><th>Size</th><th>State</th><th>Reports</th><th>Actions</th></tr></thead>
                    <tbody><For each={pack.members}>{(member) => {
                      const row = () => maps().get(member.map_id);
                      return (
                        <tr>
                          <td>{memberName(pack, member)} <span class="adm-mono" title={member.map_id}>{shortHash(member.map_id)}</span></td>
                          <td>{mapTitle(member.base.map)}</td><td>{formatBytes(member.size)}</td>
                          <td>{row()?.state ?? "–"}</td><td>{row()?.reports ?? "–"}</td>
                          <td><Show when={row()}>{(m) => <ModerationButtons target="map" id={member.map_id} state={m().state} busy={p.busy} onAction={p.onAction} />}</Show></td>
                        </tr>
                      );
                    }}</For></tbody>
                  </table>
                </td></tr>
              </Show>
            </tbody>
          );
        }}</For>
        <Show when={standalone().length}>
          <tbody><For each={standalone()}>{(m) => (
            <tr classList={{ off: m.state !== "ok" }}>
              <th scope="row" title={m.map_id}>{m.name}</th><td>{modMakers(m) || "Uncredited"}</td><td>1</td><td>{formatBytes(m.size)}</td>
              <td class={`adm-state adm-state--${m.state}`}>{m.state}</td><td>{m.reports}</td>
              <td><ModerationButtons target="map" id={m.map_id} state={m.state} busy={p.busy} onAction={p.onAction} /></td>
            </tr>
          )}</For></tbody>
        </Show>
      </table>
    </div>
  );
}
