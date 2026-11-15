// The raster art pack in web/public/art (see ART.md there): original
// emblems, medals and rank badges. Each file has a 1x and an @2x WebP.

const EMBLEMS = [
  "01-double-arch", "02-pebble-stack", "03-folded-ribbon", "04-nested-waves", "05-notched-bars", "06-rising-hooks",
  "07-maze", "08-stepping-stones", "09-crescent-dot", "10-doorways", "11-three-fins", "12-pebble-orbit",
  "13-pillars", "14-open-link", "15-stepped-bridge", "16-paired-drops", "17-half-disc-stack", "18-capsule-dots",
  "19-curved-fan", "20-wave-bands", "21-roof-stack", "22-folded-loop", "23-river", "24-offset-orbits",
];

/** Rank badges by rank name (the ladder in state/derive.ts). */
const RANKS: Record<string, string> = {
  Newcomer: "01-newcomer", Regular: "02-regular", Contender: "03-contender", Veteran: "04-veteran",
  Ace: "05-ace", Headliner: "06-headliner", Legend: "08-legend",
};

export interface ArtImage { src: string; srcset: string }

/** `size` is the CSS size it will be shown at; `native` the file's 1x size. */
function image(path: string, size: number, native: number): ArtImage {
  const one = `/art/${path}.webp`, two = `/art/${path}@2x.webp`;
  return size > native ? { src: two, srcset: `${two} 1x` } : { src: one, srcset: `${one} 1x, ${two} 2x` };
}

export function emblemArt(index: number, size: number): ArtImage {
  return image(`emblems/${EMBLEMS[((index % EMBLEMS.length) + EMBLEMS.length) % EMBLEMS.length]}`, size, 128);
}

/** Medal files are named after the medal ids. */
export function medalArt(id: string, size: number): ArtImage {
  return image(`medals/${id}`, size, 64);
}

export function rankArt(name: string, size: number): ArtImage | null {
  const f = RANKS[name];
  return f ? image(`ranks/${f}`, size, 64) : null;
}
