#!/usr/bin/env python3
"""The gate a branch passes before it merges (docs/dev/build-speed.md).

    scripts/gate.py                         gate HEAD against master, here
    scripts/gate.py --pool ~/Library/Caches/h2recomp/pool
    scripts/gate.py --dry-run               print the plan; run no cargo
    scripts/gate.py --pool DIR --exec cargo test -p d3d

Steps, in order:

1. `cargo check -p cli` with the default features, and again with
   `--no-default-features --features host`.
2. `cargo test -p <crate>` for every crate the branch can have changed
   (scripts/affected.py), dependencies first.
3. The disc differentials, when the branch touches a crate they cover.

Every step ends as one of:

    PASS         it ran and succeeded
    LEDGER       the same inputs passed before, in any checkout
    NOT NEEDED   the branch cannot have changed what the step covers
    NOT RUN      left out by a flag on this command line, which is named
    FAIL         it failed, timed out, or could not be run at all

The exit status is 0 only when no step is FAIL. A step that cannot run
(no disc, no disk space, a tool missing) is FAIL, never a quiet skip.

With `--pool DIR` the gate leases one of the pool's slots for its whole
run: a checkout (`DIR/gate-N`, a git worktree) moved to this commit, and
that checkout's own `target`. Two gates never share a target directory,
and a slot's target is only ever built from its own tree. The target is
wherever a new worktree's `target` goes on this machine: a post-checkout
hook may make it a link to another volume, and the gate builds through
it. `CARGO_TARGET_DIR` is not passed on, since one target shared by
several trees is what the pool exists to avoid.
"""

import argparse
import fcntl
import hashlib
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import affected  # noqa: E402

# The disc differentials: (label, crates whose change calls for the step,
# cargo arguments, extra environment). Each needs the retail XBE.
DISC_STEPS = [
    (
        "engine differentials",
        ("engine", "emit", "runtime"),
        ("test", "--locked", "-p", "engine", "--features", "differential"),
        {},
    ),
    (
        "native-render differentials",
        ("native-render", "engine", "emit", "runtime"),
        ("test", "--locked", "-p", "native-render", "--features", "differential"),
        {},
    ),
    (
        "runtime on flat RAM",
        ("runtime", "emit"),
        ("test", "--locked", "-p", "runtime"),
        {"H2_EMIT_FLAT_RAM": "1", "H2_FLAT_RAM": "1"},
    ),
    (
        "emit on flat RAM",
        ("emit", "runtime"),
        ("test", "--locked", "-p", "emit", "--test", "fast_copy", "--test", "perf", "--test", "differential", "--test", "second_executor"),
        {"H2_EMIT_FLAT_RAM": "1"},
    ),
]

# Environment that changes what cargo builds or what the tests do, so it
# is part of a ledger key.
KEYED_ENV = re.compile(r"^(RUSTFLAGS|RUSTDOCFLAGS|CARGO_ENCODED_RUSTFLAGS|CARGO_PROFILE_\w+|CARGO_BUILD_\w+|H2_\w+|H1_\w+)$")

FAILURE_LINE = re.compile(r"^test .* FAILED|^error|panicked at|could not compile|^failures:")
RESULT_LINE = re.compile(r"^test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored")


def say(text=""):
    print(text, flush=True)


def default_cache():
    if sys.platform == "darwin":
        return os.path.expanduser("~/Library/Caches/h2recomp")
    return os.path.join(os.environ.get("XDG_CACHE_HOME") or os.path.expanduser("~/.cache"), "h2recomp")


def free_gib(path):
    while not os.path.exists(path):
        path = os.path.dirname(path)
    return shutil.disk_usage(path).free / 2**30


def file_identity(path):
    """A file's contents when small, its size and time when not."""
    st = os.stat(path)
    if st.st_size <= 64 << 20:
        h = hashlib.sha256()
        with open(path, "rb") as f:
            for block in iter(lambda: f.read(1 << 20), b""):
                h.update(block)
        return h.hexdigest()
    return "%d@%d" % (st.st_size, int(st.st_mtime))


class Step:
    def __init__(self, label, crates, cargo_args, env=None, needs_disc=False):
        self.label = label
        self.crates = crates  # the crates whose content decides the result
        self.args = tuple(cargo_args)
        self.env = dict(env or {})
        self.needs_disc = needs_disc
        self.status = None
        self.detail = ""
        self.seconds = 0.0

    def end(self, status, detail=""):
        self.status, self.detail = status, detail
        say("    %s%s" % (status, ": " + detail if detail else ""))


class Gate:
    def __init__(self, args):
        self.args = args
        self.root = affected.repo_root()
        self.ws = affected.Workspace(self.root)
        self.cwd = self.root
        self.env = dict(os.environ)
        self.steps = []
        self.log_dir = args.log_dir or tempfile.mkdtemp(prefix="h2-gate-")
        os.makedirs(self.log_dir, exist_ok=True)
        self.lease = None
        self.toolchain = None

    # The plan.

    def disc(self):
        """The directory with the retail disc files, or None."""
        d = self.args.disc
        if d is None:
            common = affected.git(self.root, "rev-parse", "--path-format=absolute", "--git-common-dir").strip()
            d = os.path.join(os.path.dirname(common), "disc")
        return d if os.path.isfile(os.path.join(d, "default.xbe")) else None

    def plan(self):
        a = self.args
        files = self.ws.changed_files(a.base, "HEAD", worktree=True)
        crates, why, unmapped = self.ws.affected(files)
        if a.all:
            crates = set(self.ws.dir)
        if a.only:
            unknown = sorted(set(a.only) - set(self.ws.dir))
            if unknown:
                raise SystemExit("gate.py: --only names no crate: " + ", ".join(unknown))
            crates = set(a.only)
        self.affected = crates
        say("gate: %s at %s against %s" % (self.root, affected.git(self.root, "rev-parse", "--short", "HEAD").strip(), a.base))
        say("gate: %d files changed, %d crates changed, %d crates to test" % (len(files), len(why), len(crates)))
        if unmapped:
            say("gate: %d changed files belong to no crate (not built, not tested): %s%s"
                % (len(unmapped), ", ".join(unmapped[:6]), ", ..." if len(unmapped) > 6 else ""))

        disc = self.disc()
        xbe = {"H2_XBE": os.path.join(disc, "default.xbe")} if disc else {}
        if disc:
            say("gate: disc files in %s" % disc)
        else:
            say("gate: NO DISC (no default.xbe): tests that read the XBE skip themselves, and the disc steps cannot run")

        cli = "cli" in crates
        for label, extra in (("check cli", ()), ("check cli, host features", ("--no-default-features", "--features", "host"))):
            s = Step(label, ("cli",), ("check", "--locked", "-p", "cli") + extra)
            if not cli:
                s.status, s.detail = "NOT NEEDED", "cli is not affected"
            self.steps.append(s)
        for c in self.ws.order(crates):
            s = Step("test " + c, (c,), ("test", "--locked", "-p", c), xbe)
            if c in a.exclude:
                s.status, s.detail = "NOT RUN", "--exclude " + c
            self.steps.append(s)
        for label, trigger, cargo_args, env in DISC_STEPS:
            s = Step(label, (cargo_args[cargo_args.index("-p") + 1],), cargo_args, dict(xbe, **env), needs_disc=True)
            if not (set(trigger) & crates):
                s.status, s.detail = "NOT NEEDED", "none of %s is affected" % ", ".join(trigger)
            elif a.without_disc:
                s.status, s.detail = "NOT RUN", "--without-disc: " + a.without_disc
            self.steps.append(s)

    # The ledger.

    def key(self, step, keys):
        """The ledger key of `step`, or None when its inputs are not all
        committed."""
        h = hashlib.sha256()
        for c in step.crates:
            if keys.get(c) is None:
                return None
            h.update((c + "=" + keys[c] + "\n").encode())
        h.update(("cargo " + " ".join(step.args) + "\n").encode())
        env = dict(self.env, **step.env)
        for k in sorted(env):
            if KEYED_ENV.match(k) and k != "H2_XBE":
                h.update((k + "=" + env[k] + "\n").encode())
        if "H2_XBE" in env:
            h.update(("xbe=" + file_identity(env["H2_XBE"]) + "\n").encode())
        h.update((self.toolchain + "\n").encode())
        return h.hexdigest()[:32]

    # Running.

    def take_lease(self):
        """Lease a pool slot: its lock, its tree at this commit, its target."""
        a = self.args
        dirty = affected.git(self.root, "status", "--porcelain", "--untracked-files=all").strip()
        if dirty:
            raise SystemExit("gate.py: --pool builds a commit, and this checkout has uncommitted files:\n" + dirty)
        commit = affected.git(self.root, "rev-parse", "HEAD").strip()
        os.makedirs(a.pool, exist_ok=True)
        waited = 0
        while self.lease is None:
            for n in range(1, a.slots + 1):
                f = open(os.path.join(a.pool, "gate-%d.lock" % n), "a+")
                try:
                    fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
                except OSError:
                    f.close()
                    continue
                self.lease = (n, f)
                break
            else:
                if waited % 60 == 0:
                    say("gate: all %d slots of %s are leased; waiting" % (a.slots, a.pool))
                time.sleep(5)
                waited += 5
        n, f = self.lease
        f.seek(0)
        f.truncate()
        f.write("pid %d, %s at %s, since %s\n" % (os.getpid(), self.root, commit[:12], time.strftime("%F %T")))
        f.flush()
        tree = os.path.join(a.pool, "gate-%d" % n)
        if not os.path.isdir(tree):
            # A new worktree: the machine's post-checkout hook, if any,
            # places its `target` now.
            affected.git(self.root, "worktree", "prune")
            affected.git(self.root, "worktree", "add", "--detach", tree, commit)
        else:
            if affected.git(tree, "status", "--porcelain").strip():
                raise SystemExit("gate.py: %s has local changes; a pool tree is only ever checked out" % tree)
            affected.git(tree, "checkout", "--detach", "--quiet", commit)
        self.cwd = tree
        if self.env.pop("CARGO_TARGET_DIR", None):
            say("gate: CARGO_TARGET_DIR is set and is not used with --pool")
        say("gate: leased slot %d: %s, target %s" % (n, tree, self.target_dir()))

    def target_dir(self):
        return self.env.get("CARGO_TARGET_DIR") or os.path.realpath(os.path.join(self.cwd, "target"))

    def command(self, argv, env, log):
        """Run `argv` to the end or to the time limit. (exit status or None
        on a timeout, seconds)."""
        start = time.time()
        with open(log, "w") as out:
            out.write("$ %s\n" % " ".join(argv))
            out.flush()
            p = subprocess.Popen(argv, cwd=self.cwd, env=env, stdout=out, stderr=subprocess.STDOUT, start_new_session=True)
            try:
                rc = p.wait(timeout=self.args.timeout_min * 60)
            except subprocess.TimeoutExpired:
                os.killpg(p.pid, signal.SIGTERM)
                try:
                    p.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    os.killpg(p.pid, signal.SIGKILL)
                    p.wait()
                rc = None
            except BaseException:
                os.killpg(p.pid, signal.SIGTERM)
                raise
        return rc, time.time() - start

    def run_step(self, index, step, keys):
        a = self.args
        say("[%d/%d] %s" % (index, len(self.steps), step.label))
        if step.status:
            say("    %s: %s" % (step.status, step.detail))
            return
        shown = " ".join("%s=%s" % kv for kv in sorted(step.env.items()))
        say("    $ %scargo %s" % (shown + " " if shown else "", " ".join(step.args)))
        if a.dry_run:
            step.status, step.detail = "DRY RUN", "not run"
            return
        if step.needs_disc and "H2_XBE" not in step.env:
            return step.end("FAIL", "needs the retail XBE and there is no disc; --disc DIR, or --without-disc REASON to leave these out")
        free = free_gib(self.target_dir())
        if free < a.min_free_gib:
            return step.end("FAIL", "%.1f GiB free, under the %g GiB floor; nothing was built" % (free, a.min_free_gib))

        key = None if a.no_ledger else self.key(step, keys)
        lock = None
        passed = os.path.join(a.ledger, "%s.pass" % key) if key else None
        if key:
            os.makedirs(a.ledger, exist_ok=True)
            # One runner per key: a second gate waits here, then reads the
            # first one's result.
            lock = open(os.path.join(a.ledger, "%s.lock" % key), "a+")
            fcntl.flock(lock, fcntl.LOCK_EX)
            if os.path.exists(passed):
                with open(passed) as f:
                    step.end("LEDGER", f.readline().strip())
                lock.close()
                return
        elif not a.no_ledger:
            say("    (uncommitted changes in its inputs: not using the ledger)")

        log = os.path.join(self.log_dir, "%02d-%s.log" % (index, re.sub(r"\W+", "-", step.label)))
        argv = ["cargo"] + list(step.args)
        if a.jobs:
            argv += ["--jobs", str(a.jobs)]
        rc, step.seconds = self.command(argv, dict(self.env, **step.env), log)
        counts = [0, 0, 0]
        failures = []
        with open(log, errors="replace") as f:
            for line in f:
                m = RESULT_LINE.match(line)
                if m:
                    counts = [x + int(y) for x, y in zip(counts, m.groups())]
                elif FAILURE_LINE.search(line):
                    failures.append(line.rstrip())
        took = "%dm%02ds" % divmod(int(step.seconds), 60)
        ran = "" if step.args[0] != "test" else ", %d passed, %d ignored" % (counts[0], counts[2])
        if rc == 0:
            if key:
                with open(passed, "w") as f:
                    f.write("passed %s in %s at %s: cargo %s\n" % (time.strftime("%F %T"), self.cwd, self.commit, " ".join(step.args)))
            step.end("PASS", took + ran)
        else:
            why = "timed out after %g min" % a.timeout_min if rc is None else "exit %d" % rc
            step.end("FAIL", "%s, %s; log %s" % (why, took, log))
            for line in failures[:30]:
                say("      " + line)
        if lock:
            lock.close()

    def run(self):
        a = self.args
        self.commit = affected.git(self.root, "rev-parse", "--short=12", "HEAD").strip()
        if a.exec:
            if a.pool:
                self.take_lease()
            say("gate: $ " + " ".join(a.exec))
            if a.dry_run:
                return 0
            rc, _ = self.command(a.exec, self.env, os.path.join(self.log_dir, "exec.log"))
            with open(os.path.join(self.log_dir, "exec.log"), errors="replace") as f:
                sys.stdout.write(f.read())
            if rc is None:
                say("gate: FAIL: timed out after %g min" % a.timeout_min)
            return 1 if rc is None else rc

        self.plan()
        needed = [s for s in self.steps if s.status is None]
        keys = {}
        if needed and not a.dry_run:
            if a.pool:
                self.take_lease()
            else:
                say("gate: no --pool: building here, in %s; nothing keeps another cargo out of it" % self.target_dir())
            self.toolchain = subprocess.run(("rustc", "-vV"), check=True, stdout=subprocess.PIPE, universal_newlines=True).stdout
            self.toolchain += subprocess.run(("uname", "-sm"), check=True, stdout=subprocess.PIPE, universal_newlines=True).stdout
            if not a.no_ledger:
                keys = self.ws.keys()
        say("gate: logs in %s" % self.log_dir)
        for i, step in enumerate(self.steps, 1):
            self.run_step(i, step, keys)
            if step.status == "FAIL" and a.fail_fast:
                for rest in self.steps[i:]:
                    if rest.status is None:
                        rest.status, rest.detail = "FAIL", "not reached: --fail-fast stopped at '%s'" % step.label
                break
        return self.report()

    def report(self):
        say()
        say("gate: summary")
        width = max(len(s.label) for s in self.steps)
        for s in self.steps:
            if s.status != "NOT NEEDED" or self.args.verbose:
                say("  %-*s  %-10s %s" % (width, s.label, s.status, s.detail))
        count = {}
        for s in self.steps:
            count[s.status] = count.get(s.status, 0) + 1
        say("  " + ", ".join("%d %s" % (n, k) for k, n in sorted(count.items())))
        if self.lease or not self.args.dry_run:
            target = self.target_dir()
            if os.path.isdir(target):
                say("  %.1f GiB free; target %s" % (free_gib(target), target))
        if count.get("FAIL"):
            say("gate: FAILED")
            return 1
        if count.get("DRY RUN"):
            say("gate: dry run, nothing was built or tested")
            return 0
        if count.get("NOT RUN"):
            say("gate: PASSED, WITH %d STEPS LEFT OUT BY FLAGS (listed above)" % count["NOT RUN"])
            return 0
        say("gate: PASSED")
        return 0


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--base", default="master", help="the branch this one merges into (default master)")
    ap.add_argument("--all", action="store_true", help="test every crate, whatever changed")
    ap.add_argument("--only", type=lambda s: s.split(","), help="test exactly these crates (comma separated)")
    ap.add_argument("--exclude", type=lambda s: s.split(","), default=[], help="leave these crates' tests out; reported as NOT RUN")
    ap.add_argument("--pool", help="lease a tree and a target directory from this pool instead of building here")
    ap.add_argument("--slots", type=int, default=2, help="how many slots the pool has (default 2)")
    ap.add_argument("--ledger", default=os.path.join(default_cache(), "gate-ledger"), help="where passes are recorded")
    ap.add_argument("--no-ledger", action="store_true", help="run every step, and record nothing")
    ap.add_argument("--disc", help="the directory holding default.xbe (default: disc/ in the main checkout)")
    ap.add_argument("--without-disc", metavar="REASON", help="leave the disc steps out; reported as NOT RUN with the reason")
    ap.add_argument("--timeout-min", type=float, default=60, help="time limit for one step (default 60)")
    ap.add_argument("--min-free-gib", type=float, default=6, help="fail a step rather than start it under this much free disk")
    ap.add_argument("--jobs", type=int, help="cargo --jobs")
    ap.add_argument("--fail-fast", action="store_true", help="stop at the first failing step; the rest are FAIL, not reached")
    ap.add_argument("--log-dir", help="where each step's output goes (default: a new temporary directory)")
    ap.add_argument("--dry-run", action="store_true", help="print the plan and run no cargo")
    ap.add_argument("--verbose", action="store_true", help="list NOT NEEDED steps in the summary too")
    ap.add_argument("--exec", nargs=argparse.REMAINDER, help="run this command in a leased slot instead of the gate's steps")
    args = ap.parse_args()
    if args.exec is not None and not args.exec:
        ap.error("--exec needs a command")
    if args.pool:
        args.pool = os.path.abspath(os.path.expanduser(args.pool))
    gate = Gate(args)
    try:
        return gate.run()
    except KeyboardInterrupt:
        say("gate: FAILED (interrupted)")
        return 130


if __name__ == "__main__":
    sys.exit(main())
