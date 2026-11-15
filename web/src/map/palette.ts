// Map and team colours come from CSS custom properties (the theme layer),
// read once per canvas mount.

export interface MapPalette {
  bg: string;
  grid: string;
  floor: string;
  floorEdge: string;
  floorOther: string;
  label: string;
  labelBg: string;
  death: string;
  font: string;
  teams: string[];
  ffa: string[];
}

const FALLBACK: MapPalette = {
  bg: "#fff8ef", grid: "rgba(90,60,80,0.10)", floor: "#ffe7cf", floorEdge: "#e6b48f",
  floorOther: "rgba(230,180,143,0.25)", label: "#3b2a3a", labelBg: "rgba(255,255,255,0.85)", death: "#3b2a3a", font: "system-ui, sans-serif",
  teams: ["#ff6f61", "#4aa3ff", "#3cc48f", "#ffb627", "#a07cff", "#ff8fc7", "#5ad1e0", "#9b8a7a"],
  ffa: ["#ff6f61", "#4aa3ff", "#3cc48f", "#ffb627", "#a07cff", "#ff8fc7", "#5ad1e0", "#c98b5c",
        "#e85d9b", "#6b8cff", "#8bd450", "#ff9152", "#7a6cff", "#40c4aa", "#d4a24a", "#b06cd9"],
};

export function readPalette(el?: Element): MapPalette {
  if (typeof getComputedStyle === "undefined") return FALLBACK;
  const cs = getComputedStyle(el ?? document.documentElement);
  const v = (name: string, fb: string) => cs.getPropertyValue(name).trim() || fb;
  const list = (prefix: string, fb: string[]) => fb.map((c, i) => v(`${prefix}-${i}`, c));
  return {
    bg: v("--map-bg", FALLBACK.bg),
    grid: v("--map-grid", FALLBACK.grid),
    floor: v("--map-floor", FALLBACK.floor),
    floorEdge: v("--map-floor-edge", FALLBACK.floorEdge),
    floorOther: v("--map-floor-other", FALLBACK.floorOther),
    label: v("--map-label", FALLBACK.label),
    labelBg: v("--map-label-bg", FALLBACK.labelBg),
    death: v("--map-death", FALLBACK.death),
    font: v("--font-ui", FALLBACK.font),
    teams: list("--team", FALLBACK.teams),
    ffa: list("--ffa", FALLBACK.ffa),
  };
}

export function playerColour(p: MapPalette, teamPlay: boolean, team: number, index: number): string {
  return teamPlay ? p.teams[((team % p.teams.length) + p.teams.length) % p.teams.length]! : p.ffa[index % p.ffa.length]!;
}
