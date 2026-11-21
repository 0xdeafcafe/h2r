// The same HTTP moderation contract runs against native Live and Wrangler.
// Invoke directly for native, or through smoke/run.sh for the Worker.
import { deflateSync } from "node:zlib";
import { createAccount, type LocalAccount } from "../../../../web/src/state/account";
import { eventBody, signEvent } from "../../../../web/src/state/sign";
import { canonicalJson } from "../../../../web/src/state/verify";
import { issueToken, randomId } from "../../../../web/src/admin/token";

const bytes = (text: string) => new TextEncoder().encode(text);
const sha256 = async (data: Uint8Array<ArrayBuffer>) =>
  Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", data)), (b) =>
    b.toString(16).padStart(2, "0")).join("");

function solidPng(seed: string): Uint8Array<ArrayBuffer> {
  function chunk(name: string, data: Uint8Array) {
    const payload = bytes(name);
    const out = new Uint8Array(12 + data.length);
    const view = new DataView(out.buffer);
    view.setUint32(0, data.length);
    out.set(payload, 4);
    out.set(data, 8);
    let crc = 0xffffffff;
    for (const byte of out.subarray(4, out.length - 4)) {
      crc ^= byte;
      for (let bit = 0; bit < 8; bit++) crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0);
    }
    view.setUint32(out.length - 4, ~crc);
    return out;
  }
  const hdr = new Uint8Array([0, 0, 0, 2, 0, 0, 0, 1, 8, 2, 0, 0, 0]);
  const rgb = seed.slice(0, 12).match(/../g)!.map((value) => parseInt(value, 16));
  return new Uint8Array([
    137, 80, 78, 71, 13, 10, 26, 10,
    ...chunk("IHDR", hdr),
    ...chunk("IDAT", deflateSync(new Uint8Array([0, ...rgb]))),
    ...chunk("IEND", new Uint8Array()),
  ]);
}

const ownerTag = () => "Shot" + Math.floor(Math.random() * 1e8).toString(36);

export async function screenshotParity(
  api: string, aud: string, owner: LocalAccount, reporter: LocalAccount, admin: LocalAccount,
) {
  const writeScope = "content.write";
  const adminScope = "content.moderate";
  async function call(
    path: string, expected: number, init: RequestInit = {}, user?: LocalAccount, scope = writeScope,
  ) {
    const headers = new Headers(init.headers);
    if (user) headers.set("authorization", "Bearer " + await issueToken(user, aud, scope));
    const reply = await fetch(api + path, { ...init, headers });
    if (reply.status !== expected) {
      throw new Error(`${init.method ?? "GET"} ${path}: expected ${expected}, got ${reply.status} ${await reply.text()}`);
    }
    return reply;
  }

  // Invented patch bytes and map hash. No map files or game data are needed.
  const patch = bytes("synthetic screenshot parity fixture " + randomId());
  const patchHash = await sha256(patch);
  const base = { map: "lockout", sha256: "31".repeat(32), derived: false };
  const mapId = await sha256(bytes(canonicalJson({ base, format: 1, pack: [], patch: [patchHash] })));
  await call(`/content/${patchHash}`, 201, { method: "PUT", body: patch }, owner);
  const manifest = {
    base, package: { format: 1, name: "Safety smoke" },
    patch: [{ file: "smoke.bps", sha256: patchHash }],
  };
  const event = eventBody(owner, {
    stream: `package-${mapId}`, type: "MapPublished", data: { manifest },
  }, Date.now());
  await call("/maps", 201, {
    method: "PUT", body: await signEvent(owner, event),
    headers: { "content-type": "application/jose" },
  }, owner);

  // Tiny synthetic PNG with different pixels each time against a persistent server.
  const png = solidPng(patchHash);
  const digest = await sha256(png);
  const path = `/maps/${mapId}`;
  const shot = `${path}/screenshots/capture/0`;
  const permit = await (await call(`${path}/screenshot-capture-session`, 201, { method: "POST" }, owner)).json();
  const proof = {
    game_id: permit.game_id, permit: permit.token, tick: 52, map_id: mapId, map: base.map,
    camera: { position: [0, 0, 0], forward: [1, 0, 0], up: [0, 0, 1] },
    digest,
  };
  const upload = { method: "PUT", body: png };
  await call(shot, 400, upload, owner);
  await call(`${path}/screenshots/upload/0`, 400, upload, owner);
  await call(shot, 400, {
    ...upload, headers: { "x-h2-capture-proof": JSON.stringify({ ...proof, digest: "00".repeat(32) }) },
  }, owner);
  const signedUpload = { ...upload, headers: { "x-h2-capture-proof": JSON.stringify(proof) } };
  await call(shot, 403, signedUpload, reporter);
  const received = await (await call(shot, 200, signedUpload, owner)).json();
  if (received.status !== "pending") throw new Error("A private capture must await review.");
  const hash: string = received.sha256;
  const image = `${path}/images/0?v=${hash}`;
  const adminPath = `/admin/screenshots/${hash}`;
  await call(image, 404);
  await call(`/content/${hash}`, 404);
  await call(`${adminPath}/image`, 403, {}, owner, adminScope);
  async function revision(): Promise<string> {
    const pending = await (await call("/admin/screenshots", 200, {}, admin, adminScope)).json();
    const entry = pending.items.find((row: { sha256: string }) => row.sha256 === hash);
    if (!entry) throw new Error("The review queue did not include the hidden capture.");
    return entry.revision;
  }
  const preview = await call(`${adminPath}/image`, 200, {}, admin, adminScope);
  if (preview.headers.get("content-type") !== "image/webp" ||
      preview.headers.get("cache-control") !== "private, no-store" ||
      await sha256(new Uint8Array(await preview.arrayBuffer())) !== hash) {
    throw new Error("The admin preview did not return the private normalized image.");
  }
  await call(`${adminPath}/approve?revision=${await revision()}`, 200, { method: "POST" }, admin, adminScope);
  await call(image, 200);
  await call(`/screenshots/${hash}/report`, 200, { method: "POST" }, reporter);
  await call(image, 404);
  await call(`/content/${hash}`, 404);
  await call(`${adminPath}/remove?revision=${await revision()}`, 200, { method: "POST" }, admin, adminScope);
  await call(shot, 410, signedUpload, owner);
  return { pending: true, reported: true, removed: true };
}

if (import.meta.main) {
  const [url, adminTag] = process.argv.slice(2);
  if (!url || !adminTag) {
    throw new Error("Usage: bun screenshot-parity.ts SERVER_URL FRESH_CONFIGURED_ADMIN_TAG");
  }
  const origin = url.replace(/\/$/, "");
  const api = origin + "/v1";
  const { id: aud } = await (await fetch(origin + "/.well-known/h2live.json")).json();
  const owner = await createAccount(api, origin, aud, ownerTag());
  const reporter = await createAccount(api, origin, aud, ownerTag());
  const admin = await createAccount(api, origin, aud, adminTag);
  console.log(JSON.stringify(await screenshotParity(api, aud, owner, reporter, admin)));
}
