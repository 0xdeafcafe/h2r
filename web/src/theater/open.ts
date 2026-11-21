import { linkServer } from "../lib/join";

export function openFilmHref(server: string, film: string, clip?: string, search = ""): string | null {
  const base = linkServer(server);
  const id = (value: string) => value.length >= 1 && value.length <= 64 && !/[^A-Za-z0-9_-]/.test(value);
  if (!base || !id(film) || (clip !== undefined && !id(clip))) return null;
  const query = new URLSearchParams({ server: base });
  if (clip !== undefined) query.set("clip", clip);
  const options = new URLSearchParams(search);
  for (const field of ["t", "seat", "camera"]) {
    const values = options.getAll(field);
    if (!values.length) continue;
    const v = values[0]!;
    if (values.length !== 1) return null;
    if (field === "camera" ? !["default", "first-person", "follow", "orbit", "free"].includes(v)
      : !v.length || /[^0-9]/.test(v) || Number(v) > (field === "t" ? 0xffffffff : 15)) return null;
    query.set(field, v);
  }
  const href = `h2recomp://theater/${film}?${query}`;
  return href.length <= 4096 ? href : null;
}
