import { describe, expect, spyOn, test } from "bun:test";
import { api, page } from "../src/api/http";
import { targetFor } from "../src/api/server";
import type { SessionFilters } from "../src/lib/sessionFilters";

describe("REST helpers", () => {
  test("lists parse items, opaque next and position", () => {
    expect(page({ items: [1, 2], next: 42, position: 9 })).toEqual({ items: [1, 2], next: "42", position: 9 });
    expect(page({ items: [], next: null })).toEqual({ items: [], next: null, position: null });
  });

  test("a relative well-known api resolves against the document URL", () => {
    expect(targetFor("http://h:1", { id: "x", api: "/v1" }).api).toBe("http://h:1/v1");
    expect(targetFor("https://h/live", { id: "x", api: "../v2" }).api).toBe("https://h/live/v2");
    expect(targetFor("https://h/live", { id: "x", api: "https://other/v1/" }).api).toBe("https://other/v1");
    expect(targetFor("https://h/live").api).toBe("https://h/live/v1");
  });

  test("open games sends filters and the abort signal with every page", async () => {
    const options: SessionFilters = {
      kind: "multiplayer", game_type: 2, clankers: false, variant: "Rockets & Swords",
      map_category: "halo_ce", has_space: true,
    };
    const controller = new AbortController();
    const seen: URL[] = [];
    const signals: (AbortSignal | null | undefined)[] = [];
    const fetch = spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
      const url = new URL(String(input));
      seen.push(url);
      signals.push(init?.signal);
      const cursor = url.searchParams.get("cursor");
      return Response.json(cursor
        ? { items: ["second"], next: null, position: 15 }
        : { items: ["first"], next: "cursor-2", position: 12, variants: ["Rockets & Swords", "Tea Time", null, ""] });
    });
    try {
      const rest = api(targetFor("http://h:1"));
      expect(await rest.sessions(controller.signal, options)).toEqual({
        items: ["first", "second"], next: null, position: 12, variants: ["Rockets & Swords", "Tea Time"],
      });
      expect(seen.map((url) => url.pathname)).toEqual(["/v1/sessions", "/v1/sessions"]);
      expect(seen.map((url) => url.searchParams.get("cursor"))).toEqual([null, "cursor-2"]);
      expect(seen.map((url) => JSON.parse(url.searchParams.get("filters")!))).toEqual([options, options]);
      expect(seen[0]?.searchParams.get("browser_variants")).toBe("true");
      expect(signals).toEqual([controller.signal, controller.signal]);

      seen.length = 0;
      expect((await rest.sessions()).variants).toBeUndefined();
      expect(seen[0]?.searchParams.has("filters")).toBe(false);
      expect(seen[0]?.searchParams.has("browser_variants")).toBe(false);
    } finally {
      fetch.mockRestore();
    }
  });

  test("an older session list omits variant choices", async () => {
    const fetch = spyOn(globalThis, "fetch").mockImplementation(async () =>
      Response.json({ items: ["open"], next: null, position: 3 }));
    try {
      const rest = api(targetFor("http://h:1"));
      const result = await rest.sessions(undefined, { kind: "coop" });
      expect(result.items).toEqual(["open"]);
      expect(result.variants).toBeUndefined();
    } finally {
      fetch.mockRestore();
    }
  });

  test("stream URL resumes after a position and leaves out GameFrame", () => {
    const a = api({ base: "http://h:1", api: "http://h:1/v1" });
    const u = new URL(a.streamUrl(1042));
    expect(u.pathname).toBe("/v1/stream");
    expect(u.searchParams.get("from")).toBe("1042");
    expect(u.searchParams.get("exclude")).toBe("GameFrame");
    expect(u.searchParams.get("frames")).toBe("packed");
    expect(u.searchParams.get("frame_version")).toBe("8");
    expect(new URL(a.streamUrl(null)).searchParams.get("from")).toBe("0");
  });
});
