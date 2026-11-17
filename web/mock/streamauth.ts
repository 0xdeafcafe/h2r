// `/v1/stream?auth=1` in the mock, as the server does it: the first message
// must be `{kind: "auth", token}`; the replay starts only after it, and any
// other first message closes the socket (4401). The mock doesn't verify the
// token (its readers all see everything), but it keeps the order, so the
// web's signed-in stream works against it.

export type AuthStep = "replay" | "close" | "ignore";

/** What a stream socket waiting for its auth message does with `raw`. */
export function streamAuthStep(waiting: boolean, raw: unknown): AuthStep {
  if (!waiting) return "ignore";
  try {
    const m = JSON.parse(String(raw)) as { kind?: unknown; token?: unknown };
    return m.kind === "auth" && typeof m.token === "string" && m.token ? "replay" : "close";
  } catch {
    return "close";
  }
}
