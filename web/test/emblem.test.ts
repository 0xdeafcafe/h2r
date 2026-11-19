import { expect, test } from "bun:test";
import { compositeEmblem, mockEmblemSpec } from "../mock/emblem";
import { emblemPath, watchHeaderEmblems } from "../src/lib/emblem";
import { emblemChoice, emblemFromAppearance, emblemPalette, type Appearance, type EmblemSpec } from "../src/lib/gameart";

// Small RGBA masks exercise transparent-but-coloured pixels, both masks on
// the same pixel, partial coverage, rounding, and clamping.
const inputs = [
  {
    width: 4, height: 1,
    bg: [255, 255, 0, 0, 0, 0, 255, 255, 255, 255, 255, 255, 200, 200, 40, 40],
    fg: [0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 120, 120, 30, 30],
    colours: [[200, 100, 0], [100, 20, 250], [120, 45, 30], [200, 100, 100]],
  },
  {
    width: 2, height: 2,
    bg: [10, 10, 246, 246, 32, 32, 128, 128, 240, 240, 70, 70, 41, 41, 19, 19],
    fg: [255, 255, 0, 0, 0, 0, 128, 128, 0, 0, 255, 255, 127, 127, 128, 128],
    colours: [[17, 34, 51], [68, 85, 102], [119, 136, 153], [170, 187, 204]],
  },
  {
    width: 1, height: 2,
    bg: [255, 0, 255, 1, 0, 255, 0, 255],
    fg: [0, 255, 0, 255, 1, 0, 1, 0],
    colours: [[255, 0, 255], [255, 255, 255], [128, 127, 0], [129, 128, 255]],
  },
] as const;

test("PNG pixel and palette goldens match the former browser algorithm", async () => {
  const manifest = {
    version: 1,
    images: Object.fromEntries(["fg/0", "fg/1", "fg/2", "bg/0", "bg/1"].map((layer) =>
      [`emblem/${layer}`, { file: `emblem/${layer}.png`, width: 1, height: 1 }])),
    palette: ["#112233", "not a colour", "445566", " #778899 ", "#aabbcc", "#daabcd"],
  };
  const seed = "Café 👋";
  const cases: (Appearance | null)[] = [
    null,
    { emblem: { fg: 2, bg: 1 }, colours: { primary: 3, secondary: 1, tertiary: 4, quaternary: 2 } },
    { emblem: { fg: 1 }, colours: { primary: 7 } },
    { colours: { tertiary: 2 } },
    { colours: { primary: 3, tertiary: 1 } },
  ];
  const palette = emblemPalette(manifest);
  const selections = cases.map((appearance) => {
    const path = emblemPath(seed, appearance);
    const e = emblemFromAppearance(palette, appearance, emblemChoice(seed, 3, 2, palette));
    return { path, fg: e.fg, bg: e.bg, colours: [e.primary, e.secondary, e.background, e.background2] };
  });
  const pixels = inputs.map((input) => {
    const [primary, secondary, background, background2] = input.colours;
    const expected = [...compositeEmblem(input.bg, input.fg, { primary, secondary, background, background2 })];
    return { ...input, expected };
  });
  const golden = Bun.file(new URL("fixtures/emblem.json", import.meta.url));
  const json = JSON.stringify({ manifest, selections, pixels }, null, 2) + "\n";
  if (process.env.H2_UPDATE_EMBLEM_GOLDEN === "1") await Bun.write(golden, json);
  expect(await golden.text()).toBe(json);
});

test("the mock matches the site's seeded, recorded, and literal colours", () => {
  const manifest = { version: 1, images: {}, palette: ["#123456", "#aabbcc", "#dbdddf"] };
  const seed = "player";
  const palette = emblemPalette(manifest);
  const base = emblemChoice(seed, 64, 32, palette);
  const appearance = { emblem: { bg: 1 }, colours: { primary: 1, tertiary: 2 } };
  const override: Partial<EmblemSpec> = { secondary: [1, 2, 3] };
  expect(mockEmblemSpec(emblemPath(seed, appearance, override), manifest))
    .toEqual({ ...emblemFromAppearance(palette, appearance, base), ...override });
});

test("the header substitutes the fallback once for an already failed image", () => {
  const fallback = "/art/emblems/01-double-arch.webp";
  const make = (naturalWidth: number) => ({
    complete: true, naturalWidth,
    dataset: { emblemFallback: fallback },
    classList: { remove: () => {} },
    src: "/art/emblem/1-2-3-4-5-6.png",
  });
  const failed = make(0), loaded = make(256);
  const doc = {
    addEventListener: () => {},
    querySelectorAll: () => [failed, loaded],
  };
  watchHeaderEmblems(doc as unknown as Document);
  expect(failed.src).toBe(fallback);
  expect(failed.dataset.emblemFallback).toBeUndefined();
  expect(loaded.src).toBe("/art/emblem/1-2-3-4-5-6.png");
});
