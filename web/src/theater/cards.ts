import type { FilmSummary, Clip } from "../api/films";
import { clock, titleCase } from "../lib/format";
import { theaterHref } from "../routes";
import { TICKS_PER_SECOND } from "../state/frames";
import { variantLabel } from "../state/gametype";
import { usDate } from "../stats/fmt";

export function filmCard(film: FilmSummary, mapName: string, clip?: Clip) {
  const gameType = film.summary?.variant_name || film.summary?.game_type || variantLabel(film.variant) || titleCase(film.mode) || "Game";
  const hz = film.ticks_per_second || TICKS_PER_SECOND;
  const seconds = clip ? (clip.to_tick - clip.from_tick) / hz : film.summary ? film.summary.duration_ticks / hz : film.duration_s;
  const result = film.result?.trim();
  return {
    href: theaterHref(film.game_id, clip?.id),
    title: clip?.title ?? mapName,
    mapName,
    gameType,
    result: result ? titleCase(result) : "Result unavailable",
    length: seconds !== null && Number.isFinite(seconds) && seconds >= 0 ? clock(seconds) : "Length unavailable",
    date: Number.isFinite(Date.parse(film.started_at)) ? usDate(film.started_at) : undefined,
  };
}
