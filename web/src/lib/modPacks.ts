// An installable release and the individually addressable maps it contains.
import { type MapAuthor, type MapSummary, type MapImage, parseMapImages, parseMapSummaries, safeSourceUrl } from "./maps";
import { type ModQuery } from "./mods";
import { statsPath } from "../routes";

export interface ModPack {
  id: string;
  name: string;
  authors: string[];
  made_by: string;
  description?: string;
  source_url?: string;
  version?: string;
  year?: number;
  advisory?: "cheating_pack" | "not_for_online";
  uploader: MapAuthor;
  members: MapSummary[];
  size: number;
  images: MapImage[];
}

export function parseModPack(raw: unknown): ModPack | null {
  if (!raw || typeof raw !== "object") return null;
  const p = raw as Record<string, unknown>;
  if (typeof p.id !== "string" || !/^[a-z0-9][a-z0-9-]{0,63}$/.test(p.id) || typeof p.name !== "string" || !p.name.trim()) return null;
  const uploader = p.uploader as Partial<MapAuthor> | null;
  if (!uploader || typeof uploader.xuid !== "string" || typeof uploader.gamertag !== "string") return null;
  const authors = Array.isArray(p.authors) ? p.authors.filter((a): a is string => typeof a === "string" && !!a.trim()).map((a) => a.trim()) : [];
  const members = parseMapSummaries(p.members);
  // Saving metadata sends the member list back. Reject a partial parse
  // rather than dropping a map the uploader meant to retain.
  if (!Array.isArray(p.members) || members.length !== p.members.length) return null;
  return {
    id: p.id, name: p.name, authors, made_by: authors.join(", "),
    uploader: { xuid: uploader.xuid, gamertag: uploader.gamertag }, members,
    size: typeof p.size === "number" && Number.isSafeInteger(p.size) && p.size >= 0 ? p.size : members.reduce((n, m) => n + m.size, 0),
    images: parseMapImages(p.images),
    description: typeof p.description === "string" ? p.description.trim() : undefined,
    version: typeof p.version === "string" ? p.version.trim() : undefined,
    year: typeof p.year === "number" && Number.isInteger(p.year) && p.year >= 1990 && p.year <= 9999 ? p.year : undefined,
    advisory: p.advisory === "cheating_pack" || p.advisory === "not_for_online" ? p.advisory : undefined,
    source_url: safeSourceUrl(p.source_url) ?? undefined,
  };
}

export function packAdvisoryText(advisory: ModPack["advisory"]) {
  if (advisory === "cheating_pack") return "Cheating pack";
  if (advisory === "not_for_online") return "Not for online play";
  return "";
}

export const packHref = (id: string) => statsPath(`/mods/packs/${encodeURIComponent(id)}`);
export const packPath = (id: string) => `/mod-packs/${encodeURIComponent(id)}`;
export const mapCount = (n: number) => `${n} ${n === 1 ? "map" : "maps"}`;
export const packDownloads = (p: ModPack) => p.members.reduce((n, m) => n + m.downloads, 0);

export function memberName(pack: ModPack, member: MapSummary): string {
  const prefix = pack.name + ":";
  return member.name.toLowerCase().startsWith(prefix.toLowerCase()) ? member.name.slice(prefix.length).trim() || member.name : member.name;
}

export function filterPacks(packs: readonly ModPack[], q: ModQuery): ModPack[] {
  const term = q.q?.toLowerCase();
  const matches = packs.filter((p) => {
    if (q.author && p.uploader.gamertag.toLowerCase() !== q.author.toLowerCase()) return false;
    if (!p.members.some((m) => (!q.base || m.base.map.toLowerCase() === q.base) && (!q.category || m.category === q.category))) return false;
    return !term || [p.name, p.description, p.made_by, p.uploader.gamertag, ...p.members.flatMap((m) => [m.name, m.base.map, m.description, m.credit])]
      .some((s) => s?.toLowerCase().includes(term));
  });
  return q.sort === "popular" ? matches.sort((a, b) => packDownloads(b) - packDownloads(a)) : matches;
}

export function galleryUrl(api: string, path: string, image: MapImage, index: number): string {
  return `${api}${path}/images/${index}?v=${encodeURIComponent(image.sha256)}`;
}

export function memberCoverSources(api: string, m: MapSummary): string[] {
  const path = `/maps/${encodeURIComponent(m.map_id)}`;
  const sources = (m.images ?? []).map((img, i) => galleryUrl(api, path, img, i));
  if (m.image) sources.push(`${api}${path}/image?v=${encodeURIComponent(m.image)}`);
  return sources;
}

export function packCoverSources(api: string, p: ModPack): string[] {
  return [...p.images.map((img, i) => galleryUrl(api, packPath(p.id), img, i)), ...p.members.flatMap((m) => memberCoverSources(api, m))];
}
