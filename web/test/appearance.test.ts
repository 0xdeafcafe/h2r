// Player appearance: the wire shape telemetry will report (emblem layers
// and profile colours as palette indices), how it is normalised, how it
// becomes an emblem, and how game state keeps it.

import { beforeEach, describe, expect, test } from "bun:test";
import {
  EMBLEM_COLOURS, emblemChoice, emblemFromAppearance, emblemFromIndices, normalizeAppearance,
  type Appearance, type EmblemSpec,
} from "../src/lib/gameart";
import { applyEvent, applyEvents, applyGameSnapshot, initialState, standings } from "../src/state/reducer";
import { ev, resetPos } from "./helpers";
import type { GamePlayer } from "../src/api/types";

beforeEach(resetPos);

describe("normalizeAppearance", () => {
  test("keeps finite indices and drops everything else", () => {
    expect(normalizeAppearance(null)).toBeNull();
    expect(normalizeAppearance("blue")).toBeNull();
    expect(normalizeAppearance({})).toBeNull();
    expect(normalizeAppearance({ emblem: {}, colours: {} })).toBeNull();
    expect(normalizeAppearance({ emblem: { fg: "7" }, colours: { primary: NaN } })).toBeNull();
    expect(normalizeAppearance({ emblem: { fg: Infinity }, colours: { quaternary: null } })).toBeNull();
  });

  test("a whole appearance passes through as it is", () => {
    const a: Appearance = { emblem: { fg: 12, bg: 4 }, colours: { primary: 1, secondary: 2, tertiary: 3, quaternary: 4 } };
    expect(normalizeAppearance(a)).toEqual(a);
    expect(normalizeAppearance({ ...a, extra: true })).toEqual(a); // unknown members are not kept
  });

  test("reads the game's flat wire shape", () => {
    expect(normalizeAppearance({
      primary: 5, secondary: 9, tertiary: 2, quaternary: 11, emblem_foreground: 30, emblem_background: 7,
    })).toEqual({ emblem: { fg: 30, bg: 7 }, colours: { primary: 5, secondary: 9, tertiary: 2, quaternary: 11 } });
    expect(normalizeAppearance({ emblem_background: 0 })).toEqual({ emblem: { bg: 0 } });
  });

  test("a half-shape keeps the indices it has, without undefined members", () => {
    expect(normalizeAppearance({ emblem: { bg: 2, fg: true } })).toEqual({ emblem: { bg: 2 } });
    expect(normalizeAppearance({ colours: { quaternary: 17 } })).toEqual({ colours: { quaternary: 17 } });
    expect(normalizeAppearance({ colours: { tertiary: 0 } })).toEqual({ colours: { tertiary: 0 } });
    expect(Object.keys(normalizeAppearance({ emblem: { fg: 1 } })!.emblem!)).toEqual(["fg"]);
  });
});

describe("emblemFromAppearance", () => {
  const palette = EMBLEM_COLOURS;
  const base: EmblemSpec = emblemChoice("0009000000000001");

  test("a full appearance is the profile fields through emblemFromIndices", () => {
    // primary and secondary colour the foreground's masks, tertiary and
    // quaternary the background's.
    expect(emblemFromAppearance(palette, { emblem: { fg: 12, bg: 4 }, colours: { primary: 3, secondary: 5, tertiary: 6, quaternary: 7 } }, base))
      .toEqual(emblemFromIndices(palette, { fg: 12, bg: 4, primary: 3, secondary: 5, background: 6, background2: 7 }));
  });

  test("no appearance is the seed-derived choice, unchanged", () => {
    expect(emblemFromAppearance(palette, null, base)).toBe(base);
    expect(emblemFromAppearance(palette, undefined, base)).toBe(base);
    expect(emblemFromAppearance(palette, {}, base)).toEqual(base);
  });

  test("a partial appearance keeps the seed-derived values it leaves out", () => {
    const onlyPrimary = emblemFromAppearance(palette, { colours: { primary: 0 } }, base);
    expect(onlyPrimary.primary).toBe(palette[0]!);
    expect(onlyPrimary.fg).toBe(base.fg);
    expect(onlyPrimary.bg).toBe(base.bg);
    expect(onlyPrimary.secondary).toBe(base.secondary);
    expect(onlyPrimary.background).toBe(base.background);
    expect(onlyPrimary.background2).toBe(base.background2);

    const onlyFg = emblemFromAppearance(palette, { emblem: { fg: 40 } }, base);
    expect(onlyFg.fg).toBe(40);
    expect(onlyFg.bg).toBe(base.bg);
    expect(onlyFg.primary).toBe(base.primary);
  });

  test("colour indices wrap around the palette like the profile does", () => {
    const p4 = [palette[0]!, palette[1]!, palette[2]!, palette[3]!];
    const e = emblemFromAppearance(p4, { colours: { primary: 4, tertiary: -1 } }, base);
    expect(e.primary).toBe(p4[0]!); // 4 % 4
    expect(e.background).toBe(p4[3]!); // -1 wraps to the last colour
  });
});

describe("appearance in game state", () => {
  const APPEARANCE: Appearance = { emblem: { fg: 20, bg: 9 }, colours: { primary: 1, secondary: 4, tertiary: 2, quaternary: 0 } };

  test("roster events keep the appearance they carry", () => {
    const s = initialState();
    applyEvents(s, [
      ev("GameStarted", "game-g1", {
        game_id: "g1", map: "teacup", mode: "multiplayer", team_play: false, players: [
          { index: 0, name: "Mochi", team: 0, appearance: APPEARANCE },
          { index: 1, name: "Noodle", team: 1 },
        ],
      }),
      ev("PlayerJoined", "game-g1", { player: { index: 2, name: "Late", team: 0, appearance: { colours: { primary: 5 } } } }),
    ]);
    expect(s.games.g1!.players[0]!.appearance).toEqual(APPEARANCE);
    expect(s.games.g1!.players[1]!.appearance).toBeNull();
    expect(s.games.g1!.players[2]!.appearance).toEqual({ colours: { primary: 5 } });
    // standings carry it too, for the pages that draw emblems.
    expect(standings(s.games.g1!).find((p) => p.index === 0)!.appearance).toEqual(APPEARANCE);
    expect(standings(s.games.g1!).find((p) => p.index === 1)!.appearance).toBeNull();
  });

  test("a later roster repeat without it keeps what the game learned; a new one replaces it", () => {
    const s = initialState();
    applyEvents(s, [
      ev("GameStarted", "game-g1", { game_id: "g1", map: "teacup", mode: "multiplayer", team_play: false, players: [] }),
      ev("PlayerJoined", "game-g1", { player: { index: 0, name: "Mochi", team: 0, appearance: APPEARANCE } }),
      ev("PlayerJoined", "game-g1", { player: { index: 0, name: "Mochi", team: 0 } }),
    ]);
    expect(s.games.g1!.players[0]!.appearance).toEqual(APPEARANCE);
    applyEvent(s, ev("PlayerJoined", "game-g1", { player: { index: 0, name: "Mochi", team: 0, appearance: { emblem: { fg: 1 } } } }));
    expect(s.games.g1!.players[0]!.appearance).toEqual({ emblem: { fg: 1 } });
  });

  test("snapshot players keep it too; junk is dropped", () => {
    // A variable (not a fresh literal), so the extra member is fine for the
    // snapshot's typed players — the wire shape telemetry will send.
    const players = [
      { index: 0, name: "Mochi", team: 0, appearance: APPEARANCE },
      { index: 1, name: "Noodle", team: 1, appearance: { emblem: { fg: [3] }, colours: "red" } },
    ];
    const s = initialState();
    applyGameSnapshot(s, {
      game_id: "g2", map: "orchard", mode: "multiplayer", started_at: "2026-11-15T21:00:00Z", team_play: true,
      players: players as unknown as GamePlayer[],
    });
    expect(s.games.g2!.players[0]!.appearance).toEqual(APPEARANCE);
    expect(s.games.g2!.players[1]!.appearance).toBeNull();
  });
});
