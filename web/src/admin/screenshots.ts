// Review and report gallery screenshots by content hash (docs/live/live-api.md).
import { requireOk } from "../api/http";
import { isMapId } from "../lib/maps";
import type { LocalAccount } from "../state/account";
import { CONTENT_MODERATE, CONTENT_WRITE } from "./content";
import { issueToken } from "./token";

export interface ScreenshotSession {
  api: string;
  aud: string;
  me: LocalAccount;
}

export interface PendingScreenshot {
  sha256: string;
  revision: string;
  uploader: string;
  reason: string;
}

interface ScreenshotPage {
  items: PendingScreenshot[];
  next?: string;
}

export type ScreenshotDecision = "approve" | "remove";

function queueEntry(value: unknown): value is PendingScreenshot {
  if (!value || typeof value !== "object") return false;
  const row = value as Partial<PendingScreenshot>;
  return isMapId(row.sha256) && typeof row.revision === "string" && /^\d+$/.test(row.revision) &&
    typeof row.uploader === "string" &&
    /^[0-9a-f]{16}$/.test(row.uploader) && typeof row.reason === "string";
}

async function request(
  { api, aud, me }: ScreenshotSession,
  path: string,
  scope: string,
  accept: string,
  fetchFn: typeof fetch,
  init: RequestInit = {},
): Promise<Response> {
  const token = await issueToken(me, aud, scope);
  return requireOk(await fetchFn(api + path, {
    ...init, cache: "no-store", headers: { accept, authorization: `Bearer ${token}` },
  }));
}

function screenshotPath(sha256: string): string {
  if (!isMapId(sha256)) throw new Error("Invalid screenshot hash.");
  return `/screenshots/${sha256}`;
}

export async function fetchScreenshotQueue(
  session: ScreenshotSession, cursor?: string, fetchFn: typeof fetch = fetch,
): Promise<ScreenshotPage> {
  const query = cursor ? `?cursor=${encodeURIComponent(cursor)}` : "";
  const res = await request(session, "/admin/screenshots" + query, CONTENT_MODERATE, "application/json", fetchFn);
  const body = await res.json() as { items?: unknown; next?: unknown } | null;
  const items = body?.items;
  if (!Array.isArray(items) || !items.every(queueEntry)) {
    throw new Error("The server sent an unreadable screenshot queue.");
  }
  const next = body?.next;
  if (next != null && (typeof next !== "string" || !/^\d+$/.test(next))) {
    throw new Error("The server sent an unreadable screenshot cursor.");
  }
  return { items, next: next ?? undefined };
}

export async function fetchScreenshotPreview(
  session: ScreenshotSession, sha256: string, signal?: AbortSignal, fetchFn: typeof fetch = fetch,
): Promise<Blob> {
  const path = "/admin" + screenshotPath(sha256) + "/image";
  const res = await request(session, path, CONTENT_MODERATE, "image/webp", fetchFn, { signal });
  const blob = await res.blob();
  if (blob.type !== "image/webp" || !blob.size) throw new Error("The server sent an unreadable screenshot.");
  return blob;
}

export async function decideScreenshot(
  session: ScreenshotSession, item: PendingScreenshot, action: ScreenshotDecision, fetchFn: typeof fetch = fetch,
): Promise<void> {
  const path = "/admin" + screenshotPath(item.sha256) + "/" + action;
  await request(session, path + "?revision=" + encodeURIComponent(item.revision), CONTENT_MODERATE, "application/json", fetchFn, { method: "POST" });
}

export async function reportScreenshot(
  session: ScreenshotSession, sha256: string, fetchFn: typeof fetch = fetch,
): Promise<void> {
  await request(session, screenshotPath(sha256) + "/report", CONTENT_WRITE, "application/json", fetchFn, { method: "POST" });
}
