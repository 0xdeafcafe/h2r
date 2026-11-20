import { expect, test } from "bun:test";
import {
  decideScreenshot, fetchScreenshotPreview, fetchScreenshotQueue, reportScreenshot,
  type ScreenshotSession,
} from "../src/admin/screenshots";
import { parseMapImages } from "../src/lib/maps";
import { parseJws } from "../src/state/verify";

const context: ScreenshotSession = {
  api: "https://live.example/v1",
  aud: "live.example",
  me: {
    xuid: "0009000000000042", gamertag: "Reviewer", id: "acct:0009000000000042@live.example",
    server: "https://live.example", server_id: "live.example",
    kid: "kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k",
    key: "nWGxne_9WmC6hEr0kuwsxERJxWl7MmkZcDusAxyuf2A", x: "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo",
  },
};

const sha = "a".repeat(64);
const route = (path: string) => context.api + path;
const item = { sha256: sha, revision: "39", uploader: "0009000000000831", reason: "Reported by a player" };

function recorder(response: Response) {
  const calls: { url: string; init: RequestInit }[] = [];
  const fetchFn = (async (url: string, init: RequestInit) => {
    calls.push({ url, init });
    return response;
  }) as unknown as typeof fetch;
  return { fetchFn, calls };
}

function expectScope(init: RequestInit, scope: string) {
  const token = new Headers(init.headers).get("authorization");
  expect(token).toMatch(/^Bearer /);
  expect(parseJws(token!.slice(7))?.payload).toMatchObject({
    aud: context.aud, iss: context.me.id, lxm: scope,
  });
}

test("a queue page uses the cursor and an admin content token", async () => {
  const read = recorder(Response.json({ items: [item], next: "42" }));
  expect(await fetchScreenshotQueue(context, "100", read.fetchFn)).toEqual({ items: [item], next: "42" });
  expect(read.calls).toHaveLength(1);
  const [call] = read.calls;
  expect(call!.url).toBe(route("/admin/screenshots?cursor=100"));
  expect(call!.init.cache).toBe("no-store");
  expectScope(call!.init, "content.moderate");
});

test("unreadable queue rows and cursors surface an error", async () => {
  for (const response of [{ items: [{ ...item, uploader: 42 }] }, { items: [item], next: "later" }]) {
    const read = recorder(Response.json(response));
    await expect(fetchScreenshotQueue(context, undefined, read.fetchFn)).rejects.toThrow("unreadable screenshot");
  }
});

test("the private preview requests a WebP and rejects a different MIME type", async () => {
  // Synthetic bytes exercise the response boundary. Decoding is the browser's job.
  const read = recorder(new Response(new Uint8Array([1, 2]), { headers: { "content-type": "image/webp" } }));
  expect(await fetchScreenshotPreview(context, sha, undefined, read.fetchFn)).toMatchObject({ type: "image/webp", size: 2 });
  expect(read.calls).toHaveLength(1);
  const [call] = read.calls;
  expect(call!.url).toBe(route(`/admin/screenshots/${sha}/image`));
  expect(new Headers(call!.init.headers).get("accept")).toBe("image/webp");
  expect(call!.init.cache).toBe("no-store");
  expectScope(call!.init, "content.moderate");
  const bad = recorder(new Response("fallback HTML", { headers: { "content-type": "text/html" } }));
  await expect(fetchScreenshotPreview(context, sha, undefined, bad.fetchFn)).rejects.toThrow("unreadable screenshot");
});

test("decisions and player reports use separate scopes and stable hashes", async () => {
  for (const action of ["approve", "remove"] as const) {
    const admin = recorder(new Response(null, { status: 204 }));
    await decideScreenshot(context, item, action, admin.fetchFn);
    expect(admin.calls).toHaveLength(1);
    expect(admin.calls[0]).toMatchObject({ url: route(`/admin/screenshots/${sha}/${action}?revision=39`), init: { method: "POST" } });
    expectScope(admin.calls[0]!.init, "content.moderate");
  }
  const player = recorder(Response.json({ ok: true }));
  await reportScreenshot(context, sha, player.fetchFn);
  expect(player.calls).toHaveLength(1);
  expect(player.calls[0]).toMatchObject({ url: route(`/screenshots/${sha}/report`), init: { method: "POST" } });
  expectScope(player.calls[0]!.init, "content.write");
  await expect(reportScreenshot(context, "../oops", player.fetchFn)).rejects.toThrow("Invalid screenshot hash");
  expect(player.calls).toHaveLength(1);
});

test("gallery metadata advertises reportability only for an explicit flag and a valid hash", () => {
  expect(parseMapImages([
    { sha256: sha, reportable: true }, { sha256: sha }, { sha256: sha, reportable: "true" },
    { sha256: "untrusted", reportable: true },
  ])).toEqual([
    { sha256: sha, reportable: true }, { sha256: sha }, { sha256: sha }, { sha256: "untrusted" },
  ]);
});
