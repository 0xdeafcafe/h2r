import { describe, expect, test } from "bun:test";
import { fetchHosts, formatHostCost, formatHostUptime, hostEndReason, hostStateBadge, stopHost, type HostsAdmin } from "../src/admin/hosts";
import { dedicatedHostLabel, type HostState } from "../src/lib/hosts";
import { parseJws } from "../src/state/verify";

const CTX: HostsAdmin = {
  api: "http://h/v1",
  aud: "live.example.net",
  me: {
    xuid: "0009000000000042", gamertag: "afr", id: "acct:0009000000000042@live.example.net",
    server: "http://h", server_id: "live.example.net", kid: "kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k",
    key: "nWGxne_9WmC6hEr0kuwsxERJxWl7MmkZcDusAxyuf2A", x: "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo",
  },
};

function recorder(response: Response, steps: string[] = []) {
  const calls: { url: string; method: string; authorization: string | null; body: RequestInit["body"] }[] = [];
  const fetchFn = (async (url: string, init: RequestInit = {}) => {
    steps.push("fetch");
    calls.push({ url, method: init.method ?? "GET", authorization: new Headers(init.headers).get("authorization"), body: init.body });
    return response;
  }) as unknown as typeof fetch;
  return { fetchFn, calls };
}

describe("dedicated host display", () => {
  test("shows dollars, including free local hosts and nonzero sub-cent costs", () => {
    expect([0, 0.001, 0.01, 1.234, 1200].map(formatHostCost)).toEqual(["$0.00", "<$0.01", "$0.01", "$1.23", "$1,200.00"]);
    expect(formatHostCost(NaN)).toBe("—");
  });

  test("formats host uptime from milliseconds", () => {
    expect([0, -3000, 59000, 60000, 3900000, 90000000].map(formatHostUptime)).toEqual(["0s", "0s", "59s", "1m", "1h 05m", "1d 1h"]);
    expect(formatHostUptime(Infinity)).toBe("—");
  });

  test("badges cover every lifecycle state and distinguish failure from completion", () => {
    const states: HostState[] = ["requested", "booting", "joinable", "playing", "ended", "failed"];
    expect(states.map(hostStateBadge)).toEqual([
      { label: "Requested", tone: "warning" }, { label: "Booting", tone: "warning" },
      { label: "Joinable", tone: "good" }, { label: "Playing", tone: "good" },
      { label: "Ended", tone: "quiet" }, { label: "Failed", tone: "critical" },
    ]);
    expect(hostEndReason("boot_timeout")).toBe("Boot timed out");
    expect(hostEndReason("stopped")).toBe("Stopped");
  });

  test("public host labels require a lease and tolerate a missing region", () => {
    expect(dedicatedHostLabel({})).toBeNull();
    expect(dedicatedHostLabel({ host_lease: null, dedicated_region: "lhr" })).toBeNull();
    expect(dedicatedHostLabel({ host_lease: "hl-a" })).toBe("Dedicated");
    expect(dedicatedHostLabel({ host_lease: "hl-a", dedicated_region: "lhr" })).toBe("Dedicated (lhr)");
  });
});

describe("hosts inventory", () => {
  test("reads an empty inventory with this server's signed admin.read token", async () => {
    const data = { running: [], recent: [], month_to_date_usd: 0, budget_usd: 5 };
    const read = recorder(Response.json(data));
    expect(await fetchHosts(CTX, read.fetchFn)).toEqual(data);
    expect(read.calls).toHaveLength(1);
    expect(read.calls[0]).toMatchObject({ url: "http://h/v1/admin/hosts", method: "GET" });
    const token = read.calls[0]?.authorization;
    expect(token).toMatch(/^Bearer /);
    expect(parseJws(token!.slice(7))?.payload).toMatchObject({ aud: CTX.aud, lxm: "admin.read", iss: CTX.me.id });
  });

  test("loads active and ended rows with their metered costs", async () => {
    const lease = { id: "hl-a", provider: "fly", region: "lhr", class: "small", max_players: 8 };
    const running = { lease, state: "playing", players: 4, uptime_ms: 42000, cost_usd: 0.001 };
    const recent = { lease: { ...lease, id: "hl-b" }, state: "ended", reason: "match_over", cost_usd: 0.06 };
    const data = { running: [running], recent: [recent], month_to_date_usd: 0.061, budget_usd: 5 };
    const read = recorder(Response.json(data));
    expect(await fetchHosts(CTX, read.fetchFn)).toEqual(data);
  });

  test("reports an invalid inventory as an error", async () => {
    const read = recorder(Response.json({ running: [], recent: [], month_to_date_usd: 0 }));
    await expect(fetchHosts(CTX, read.fetchFn)).rejects.toThrow("invalid host inventory");
    const badRow = recorder(Response.json({ running: [null], recent: [], month_to_date_usd: 0, budget_usd: 5 }));
    await expect(fetchHosts(CTX, badRow.fetchFn)).rejects.toThrow("invalid host inventory");
  });
});

describe("stop host", () => {
  test("cancel sends no request", async () => {
    const post = recorder(new Response(null, { status: 204 }));
    let prompt = "";
    expect(await stopHost(CTX, "hl-one", (message) => { prompt = message; return false; }, post.fetchFn)).toBe(false);
    expect(prompt).toContain("hl-one");
    expect(post.calls).toHaveLength(0);
  });

  test("confirmation sends one signed admin.write POST for the lease and accepts 204", async () => {
    const steps: string[] = [];
    const post = recorder(new Response(null, { status: 204 }), steps);
    expect(await stopHost(CTX, "hl-a/b", () => { steps.push("confirm"); return true; }, post.fetchFn)).toBe(true);
    expect(steps).toEqual(["confirm", "fetch"]);
    expect(post.calls).toHaveLength(1);
    expect(post.calls[0]).toMatchObject({ url: "http://h/v1/admin/hosts/hl-a%2Fb/stop", method: "POST", body: undefined });
    const token = post.calls[0]?.authorization;
    expect(token).toMatch(/^Bearer /);
    expect(parseJws(token!.slice(7))?.payload).toMatchObject({ aud: CTX.aud, lxm: "admin.write", iss: CTX.me.id });
  });

  test("a refused stop preserves the server error for the page", async () => {
    const post = recorder(Response.json({ error: "forbidden", message: "Admin required" }, { status: 403 }));
    await expect(stopHost(CTX, "hl-one", () => true, post.fetchFn)).rejects.toMatchObject({
      status: 403, code: "forbidden", message: "Admin required",
    });
  });
});
