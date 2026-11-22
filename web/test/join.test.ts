import { expect, test } from "bun:test";
import { canJoinSession, friendSession, joinHref, watchLaunch } from "../src/lib/join";
import { parseSession } from "../src/lib/sessions";

const id = "0123456789abcdef";

test("links carry the Live discovery base and the session as separate fields", () => {
  expect(joinHref("HTTPS://PLAY.example:443/live/", id)).toBe(
    "h2recomp://join?server=https%3A%2F%2Fplay.example%2Flive&session=0123456789abcdef",
  );
  const local = new URL(joinHref("http://[::1]:8074", id)!);
  expect(local.searchParams.get("server")).toBe("http://[::1]:8074");
  expect(local.hostname).toBe("join");
  expect(local.searchParams.get("session")).toBe(id);
  expect(joinHref("https://play.example", id.toUpperCase())).toBeNull();
  expect(joinHref("https://play.example", "1")).toBeNull();
});

test("malformed or credential-bearing bases never become launch actions", () => {
  for (const base of ["file:///etc", "https:///host", "https://u:p@host", "https://@host",
    "https://host/#", "https://host?", "https://host/../lab", "https://host/%2e%2e/lab",
    "https://host/a\\b", " https://host", "https://host/a b", "https://host/" + "a".repeat(4096)]) {
    expect(joinHref(base, id)).toBeNull();
  }
});

test("a friend's host or guest seat identifies a single joinable lobby", () => {
  const s = parseSession({ session_id: id, host_xuid: "host", host_gamertag: "Host", players: { guest: "public" },
    advertisement: { slots: { public_open: 2, public_filled: 1 } } })!;
  expect(canJoinSession({ ...s, host_gamertag: "", host_lease: "hl-example" }, "me")).toBe(true);
  expect(canJoinSession({ ...s, host_gamertag: "", host_lease: undefined }, "me")).toBe(false);
  for (const friend of ["host", "guest"]) expect(friendSession([s], friend, "me")).toBe(s);
  for (const viewer of ["host", "guest"]) expect(canJoinSession(s, viewer)).toBe(false);
  expect(friendSession([s], "away", "me")).toBeUndefined();
  expect(friendSession([s, { ...s, session_id: "bbbbbbbbbbbbbbbb" }], "host", "me")).toBeUndefined();
  for (const unavailable of [{ ...s, state: "expired" }, { ...s, visibility: "invite" as const },
    { ...s, assigned: true }, { ...s, flags: "ranked" }, { ...s, slots: { ...s.slots, public_open: 0, private_open: 1 } },
    { ...s, players: { p1: "", p2: "", p3: "" } }]) expect(canJoinSession(unavailable, "me")).toBe(false);
});

test("the installation hint is delayed and a hand-off or unmount cancels it", async () => {
  const page = Object.assign(new EventTarget(), { hidden: false });
  const hints: string[] = [];
  watchLaunch(page, () => hints.push("opened"), 0);
  page.hidden = true;
  page.dispatchEvent(new Event("visibilitychange"));
  page.hidden = false;
  const cancel = watchLaunch(page, () => hints.push("unmounted"), 0);
  cancel();
  watchLaunch(page, () => hints.push("guide"), 0);
  expect(hints).toEqual([]);
  await new Promise((resolve) => setTimeout(resolve, 10));
  expect(hints).toEqual(["guide"]);
});
