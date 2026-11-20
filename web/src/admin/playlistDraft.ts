import { ENGINES } from "./catalog";
import {
  MAX_WEIGHT, buildPatch, clipUtf8, isUnused, MAX_TITLE, variantLabel,
  type EditHopper, type EditRecord, type PlaylistTemplate,
} from "./validate";

export function playlistTitle(hopper: EditHopper): string {
  return hopper.title || hopper.name || "Untitled playlist";
}

export function playlistKind(type: number, clanker: boolean): string {
  if (clanker) return "Clanker";
  return [2, 4, 5].includes(type) ? "Ranked" : "Social";
}

export function playlistSize({ type, ranges: [a, b, c] }: EditHopper): string {
  const range = (min: number, max: number) => min === max ? String(min) : `${min}–${max}`;
  if (type < 3) return `${range(a, b)} players · Free for all`;
  return `${a} teams · ${range(b, c)} per team · ${range(a * b, a * c)} players`;
}

export interface VariantGroup {
  key: string;
  label: string;
  slots: number[];
}

/** Group the decoded variants in display order while keeping every original slot. */
export function groupEntries(records: readonly EditRecord[]): VariantGroup[] {
  const groups = new Map<string, VariantGroup>();
  records.forEach((record, slot) => {
    const variant = record.variant;
    const key = record.pick ?? record.builtin ?? JSON.stringify(variant ? [variant.name, variant.engine, variant.team] : []);
    const group = groups.get(key);
    if (group) group.slots.push(slot);
    else groups.set(key, { key, label: variant ? variantLabel(variant, ENGINES) : "Unknown variant", slots: [slot] });
  });
  return [...groups.values()];
}

/** Weight is a signed dword to the game, even though the raw editor shows u32. */
export function effectiveWeight(weight: number): number {
  return Math.min(MAX_WEIGHT, Math.max(1, weight | 0));
}

export function entryShares(records: readonly EditRecord[]): number[] {
  const weights = records.map((record) => effectiveWeight(record.dwords[0]));
  const total = weights.reduce((sum, weight) => sum + weight, 0);
  return weights.map((weight) => 100 * weight / total);
}

export function shareLabel(share: number): string {
  return share > 0 && share < 0.1 ? "<0.1%" : `${Number(share.toFixed(1))}%`;
}

/** Move within the group's existing slots, preserving other groups' order. */
export function moveGroupEntry(records: readonly EditRecord[], slots: readonly number[], from: number, to: number): EditRecord[] {
  const entries = slots.map((slot) => records[slot]!);
  const [entry] = entries.splice(from, 1);
  entries.splice(to, 0, entry!);
  const next = [...records];
  slots.forEach((slot, i) => { next[slot] = entries[i]!; });
  return next;
}

export function duplicatePlaylist(source: EditHopper, slot: EditHopper, template: PlaylistTemplate): EditHopper {
  const suffix = " copy";
  return {
    ...source,
    index: slot.index,
    template: template.id,
    clear: false,
    name: "",
    title: clipUtf8(playlistTitle(source), MAX_TITLE - suffix.length).trimEnd() + suffix,
    records: source.records.map((record) => ({
      ...record,
      srcHopper: record.pick ? undefined : record.srcHopper ?? source.index,
    })),
  };
}

interface DiffLine { field: string; before: string | null; after: string | null }
interface PlaylistDiff { index: number; title: string; action: "Added" | "Removed" | "Edited"; lines: DiffLine[] }

function entryText(record: EditRecord, mapName: (id: number) => string): string {
  const variant = record.variant ? variantLabel(record.variant, ENGINES) : "Unknown variant";
  return `${mapName(record.id)} · ${variant} · Weight ${record.dwords[0]}`;
}

function entrySource(record: EditRecord, hopper: number, slot: number): string {
  if (record.pick) return `Built-in · ${record.pick}`;
  return `Playlist ${(record.srcHopper ?? hopper) + 1}, entry ${(record.src ?? slot) + 1}`;
}

/** Compare the edited slots the PUT will touch, including source changes and raw fields. */
export function playlistDiff(
  published: EditHopper[], draft: EditHopper[], mapName: (id: number) => string,
): PlaylistDiff[] {
  return buildPatch(published, draft).hoppers.map((patch) => {
    const before = published.find((hopper) => hopper.index === patch.index);
    const after = draft.find((hopper) => hopper.index === patch.index)!;
    const old = before && !isUnused(before) ? before : undefined;
    const next = "clear" in patch ? undefined : after;
    const lines: DiffLine[] = [];
    function change(field: string, a: string | number | undefined, b: string | number | undefined) {
      if (a !== b) lines.push({ field, before: a == null ? null : String(a), after: b == null ? null : String(b) });
    }
    if (next) {
      if (next.template != null) change("Descriptor template", undefined, next.template);
      change("Title", old?.title, next.title);
      change("Description", old?.description, next.description);
      change("Type", old?.type, next.type);
      change("Status", old?.status, next.status);
      change("Raw name", old?.name, next.name);
      change("Ranges", old?.ranges.join(" / "), next.ranges.join(" / "));
      change("Descriptor words", old?.dwords.join(", "), next.dwords.join(", "));
      for (let i = 0; i < Math.max(old?.records.length ?? 0, next.records.length); i++) {
        const a = old?.records[i];
        const b = next.records[i];
        const field = `Entry ${i + 1}`;
        change(field, a && entryText(a, mapName), b && entryText(b, mapName));
        change(`${field} source`, a && entrySource(a, after.index, i), b && entrySource(b, after.index, i));
        if (a && b && !b.pick) {
          change(`${field} string`, a.string, b.string);
          change(`${field} flags`, a.flags, b.flags);
          change(`${field} word 2`, a.dwords[1], b.dwords[1]);
        }
      }
    }
    return {
      index: patch.index,
      title: playlistTitle(next ?? old ?? after),
      action: !next ? "Removed" : old ? "Edited" : "Added",
      lines,
    };
  });
}
