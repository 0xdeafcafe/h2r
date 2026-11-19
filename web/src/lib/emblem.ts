import { fnv, type Appearance, type EmblemSpec, type Rgb } from "./gameart";

export const EMBLEM_PATH = "/art/emblem/";

const rgb = (colour: Rgb) => "r" + colour.map((v) => Math.min(255, Math.max(0, Math.round(v))).toString(16).padStart(2, "0")).join("");

/** The same appearance URL works on the first paint and on later renders.
 *  Seed selectors let the server choose from the layers and palette it has. */
export function emblemPath(seed: string, a?: Appearance | null, override?: Partial<EmblemSpec>): string {
  const hash = (salt: number) => fnv(seed, salt).toString(16).padStart(8, "0");
  const index = (value: number | undefined, max: number) =>
    value !== undefined && Number.isInteger(value) && value >= 0 && value < max ? String(value) : null;
  const colour = (value: number | undefined, literal?: Rgb) => literal?.every(Number.isFinite) ? rgb(literal) : index(value, 256);
  const primary = colour(a?.colours?.primary, override?.primary);
  const secondary = colour(a?.colours?.secondary, override?.secondary);
  const background = colour(a?.colours?.tertiary, override?.background);
  const background2 = colour(a?.colours?.quaternary, override?.background2);
  const fields = [
    index(override?.fg ?? a?.emblem?.fg, 64),
    index(override?.bg ?? a?.emblem?.bg, 32),
    primary, secondary, background, background2,
  ].map((field, i) => field ?? "s" + hash(i + 1));
  // A missing colour still depends on the original seeded choices, even
  // when the player has supplied their actual primary or background colour.
  if ((primary && (!secondary || !background || !background2)) || (background && !background2)) {
    fields.push(hash(3) + hash(5));
  }
  return EMBLEM_PATH + fields.join("-") + ".png";
}

/** Keep same-origin header URLs byte-identical to the server's markup. */
export function emblemOrigin(base: string, origin: string): string {
  return base === origin ? "" : base;
}

/** Header images exist outside Solid's tree, and may fail before JS starts. */
export function watchHeaderEmblems(doc: Document): void {
  const fallback = (img: HTMLImageElement) => {
    const src = img.dataset.emblemFallback;
    if (!src) return;
    delete img.dataset.emblemFallback;
    img.classList.remove("emblem--game");
    img.src = src;
  };
  doc.addEventListener("error", (event) => {
    if (event.target instanceof HTMLImageElement) fallback(event.target);
  }, true);
  for (const img of doc.querySelectorAll<HTMLImageElement>("img[data-emblem-fallback]")) {
    if (img.complete && img.naturalWidth === 0) fallback(img);
  }
}
