// Finds the `h2` binary that serves `h2 rules lsp`. Pure: the caller
// passes the environment in, so the tests run without VS Code.

import * as path from 'path';

/** Where a found binary came from. */
export type ServerSource = 'setting' | 'path' | 'cargo';

export type ServerLookup =
  | { found: true; path: string; source: ServerSource }
  | { found: false; tried: string[]; setting?: string };

/** Whether a file exists and may be run. */
export type ExistsFn = (file: string) => boolean;

const DEFAULT_PATHEXT = '.COM;.EXE;.BAT;.CMD';

function unique(items: string[]): string[] {
  return items.filter((item, i) => items.indexOf(item) === i);
}

function pathApi(platform: string): path.PlatformPath {
  return platform === 'win32' ? path.win32 : path.posix;
}

/** `~` or `~/x` (`~\x` on Windows) to the home directory. */
export function expandHome(value: string, home: string, platform: string = process.platform): string {
  if (value === '~') {
    return home;
  }
  if (value.startsWith('~/') || (platform === 'win32' && value.startsWith('~\\'))) {
    return pathApi(platform).join(home, value.slice(2));
  }
  return value;
}

/** The names `name` may have on disk: itself, and on Windows each PATHEXT extension. */
export function executableNames(name: string, platform: string, pathExt?: string): string[] {
  if (platform !== 'win32') {
    return [name];
  }
  const exts = (pathExt && pathExt.length > 0 ? pathExt : DEFAULT_PATHEXT)
    .split(';')
    .map((e) => e.trim())
    .filter((e) => e.length > 0);
  const lower = name.toLowerCase();
  const names = exts.map((e) => name + e.toLowerCase());
  // A name that already carries an extension is tried as written first.
  if (exts.some((e) => lower.endsWith(e.toLowerCase()))) {
    names.unshift(name);
  }
  return names;
}

/** Every place `name` is looked for on a `PATH` string, in order. */
export function pathCandidates(name: string, pathEnv: string | undefined, platform: string, pathExt?: string): string[] {
  if (!pathEnv) {
    return [];
  }
  const api = pathApi(platform);
  const sep = platform === 'win32' ? ';' : ':';
  const out: string[] = [];
  for (const raw of pathEnv.split(sep)) {
    // Windows PATH entries may be quoted.
    const dir = raw.trim().replace(/^"(.*)"$/, '$1');
    if (dir.length === 0) {
      continue;
    }
    for (const file of executableNames(name, platform, pathExt)) {
      out.push(api.join(dir, file));
    }
  }
  return out;
}

/**
 * Finds the server binary.
 *
 * 1. `setting` (`h2rules.serverPath`), with `~` expanded. A bare name
 *    with no directory in it is looked up on `PATH`. A setting that names
 *    nothing is not passed over: the lookup fails and names it.
 * 2. `h2` on `PATH` (with `PATHEXT` on Windows).
 * 3. `~/.cargo/bin/h2` (`h2.exe` on Windows).
 */
export function resolveServerPath(
  setting: string | undefined,
  pathEnv: string | undefined,
  home: string,
  exists: ExistsFn,
  platform: string = process.platform,
  pathExt?: string,
): ServerLookup {
  const api = pathApi(platform);
  const tried: string[] = [];
  const first = (candidates: string[]): string | undefined => {
    for (const c of candidates) {
      tried.push(c);
      if (exists(c)) {
        return c;
      }
    }
    return undefined;
  };

  const configured = (setting ?? '').trim();
  if (configured.length > 0) {
    const expanded = expandHome(configured, home, platform);
    const bare = !expanded.includes('/') && !(platform === 'win32' && expanded.includes('\\'));
    const full = api.normalize(expanded);
    // A path is tried as written; on Windows also with each PATHEXT
    // extension, so `C:\tools\h2` finds `C:\tools\h2.exe`.
    const candidates = bare
      ? pathCandidates(expanded, pathEnv, platform, pathExt)
      : platform === 'win32'
        ? unique([full, ...executableNames(full, platform, pathExt)])
        : [full];
    const hit = first(candidates);
    return hit ? { found: true, path: hit, source: 'setting' } : { found: false, tried, setting: configured };
  }

  const onPath = first(pathCandidates('h2', pathEnv, platform, pathExt));
  if (onPath) {
    return { found: true, path: onPath, source: 'path' };
  }

  if (home.length > 0) {
    const cargo = api.join(home, '.cargo', 'bin', platform === 'win32' ? 'h2.exe' : 'h2');
    const hit = first([cargo]);
    if (hit) {
      return { found: true, path: hit, source: 'cargo' };
    }
  }
  return { found: false, tried };
}
