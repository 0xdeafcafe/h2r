import type { GameDetail, GameSummary, PlayerScore } from "../api/types";
import { statsPath } from "../routes";

export interface SimBot {
  name?: string;
  skill?: string;
  team?: number;
  arm?: string;
  tuning?: Record<string, unknown>;
}

export interface SimMatch {
  run_id: string;
  batch_id?: string;
  seed: string;
  gametype: string;
  pace: string;
  bots: SimBot[];
  deferred: boolean;
}

export interface SimResult {
  end: string;
  winner?: { player?: number; team?: number } | null;
  teams?: { team: number; score: number }[];
}

export interface SimRun extends GameSummary {
  sim: SimMatch;
  status: "running" | "finished" | "interrupted";
  ticks: number;
  speedup: number | null;
  scoreboard: PlayerScore[];
  result: SimResult | null;
  determinism: unknown;
  has_report: boolean;
  recording_limited: boolean;
}

export interface SimPlayer {
  index: number;
  name: string;
  bot: boolean;
  arm?: string;
  score: number;
  kills: number;
  deaths: number;
  shots?: number;
  accuracy?: number;
}

export interface SimReport {
  format: "h2-sim-report/1";
  players: SimPlayer[];
  notes?: string[];
}

export interface SimDetail { run: SimRun; game: GameDetail; report: SimReport | null }
export interface SimFilter {
  map?: string;
  tuning?: string;
  tuning_choice?: string;
  batch?: string;
  before?: string;
  include_options?: 1;
}

export interface SimFilterOptions {
  maps: string[];
  arms: string[];
  batches: string[];
  custom_tuning: boolean;
  default_tuning: boolean;
}

export const TRAINING_BATCH_COMMAND = "h2 sim batch docs/runtime/sim-examples/batch.toml --parallel 4 --upload";

export function trainingHref(run?: string, batch?: string): string {
  const q = new URLSearchParams({ tab: "training" });
  if (run) q.set("run", run);
  if (batch) q.set("batch", batch);
  return `${statsPath("/admin")}?${q}`;
}

export function runsByBatch(runs: readonly SimRun[]): { id: string | null; runs: SimRun[] }[] {
  const groups = new Map<string | null, SimRun[]>();
  for (const run of runs) {
    const id = run.sim.batch_id ?? null;
    const group = groups.get(id);
    if (group) group.push(run);
    else groups.set(id, [run]);
  }
  return Array.from(groups, ([id, runs]) => ({ id, runs }));
}

export function speedLabel(speed: number | null): string {
  return speed === null ? "—" : `${speed.toFixed(2)}×`;
}

export function tuningLabel(bot: SimBot): string {
  return [bot.arm, bot.skill].filter(Boolean).join(" · ") || "Default";
}

export function determinismLabel(check: unknown): string | null {
  if (check == null) return null;
  const passed = typeof check === "object" && "passed" in check ? check.passed : check;
  if (passed === true) return "Determinism passed";
  if (passed === false) return "Determinism failed";
  return "Determinism check";
}

export function resultLabel(run: SimRun): string {
  if (!run.result) return "—";
  const { winner, end } = run.result;
  const reason = end.replaceAll("_", " ");
  if (winner?.team !== undefined) return `Team ${winner.team + 1} · ${reason}`;
  if (winner?.player !== undefined) {
    const name = run.players.find((p) => p.index === winner.player)?.name ?? `Player ${winner.player + 1}`;
    return `${name} · ${reason}`;
  }
  return reason;
}

export function scoreLabel(run: SimRun): string {
  const teams = run.result?.teams?.length ? run.result.teams : run.teams;
  if (teams?.length) return teams.map((t) => t.score).join("–");
  return run.scoreboard.map((p) => p.score).join("–") || "—";
}

export function comparisonRows(a: SimRun, b: SimRun): [string, string, string][] {
  const cells: [string, (run: SimRun) => string][] = [
    ["Map", (run) => run.map],
    ["Gametype", (run) => run.sim.gametype],
    ["Seed", (run) => run.sim.seed],
    ["Tuning", (run) => run.sim.bots.map(tuningLabel).join(", ")],
    ["Result", resultLabel],
    ["Score", scoreLabel],
    ["Kills", (run) => run.scoreboard.length ? String(run.scoreboard.reduce((n, p) => n + p.kills, 0)) : "—"],
    ["Deaths", (run) => run.scoreboard.length ? String(run.scoreboard.reduce((n, p) => n + p.deaths, 0)) : "—"],
    ["Ticks", (run) => run.ticks.toLocaleString()],
    ["Speed", (run) => speedLabel(run.speedup)],
  ];
  return cells.map(([label, cell]) => [label, cell(a), cell(b)]);
}
