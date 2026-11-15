import { describe, expect, test } from "bun:test";
import { api, page } from "../src/api/http";
import { targetFor } from "../src/api/server";

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

  test("stream URL resumes after a position and leaves out GameFrame", () => {
    const a = api({ base: "http://h:1", api: "http://h:1/v1" });
    const u = new URL(a.streamUrl(1042));
    expect(u.pathname).toBe("/v1/stream");
    expect(u.searchParams.get("from")).toBe("1042");
    expect(u.searchParams.get("exclude")).toBe("GameFrame");
    expect(new URL(a.streamUrl(null)).searchParams.get("from")).toBe("0");
  });
});
