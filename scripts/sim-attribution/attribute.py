#!/usr/bin/env python3
"""Attribute every sample of an `H2_PROFILE` run to a code family.

The method is in docs/perf/sim-attribution.md. Inputs are all local:

  --xbe        your own default.xbe (read only; a scratch ELF and its
               disassembly go under --work, never into the repository)
  --functions  the list written by `h2 analyze --functions FILE`
  --profile    a profile directory holding profile.folded
  --work       scratch directory (created; reused as a cache)
  --objdump    llvm-objdump (default: llvm-objdump on PATH)
  --ms         the unprofiled ms/tick of the same case, to print ms columns
  --log        the run's stderr log, to read hooked points from its
               `feature plan:` lines (optional)

Run it with `python3 -I` from outside the scratch directory. It prints
plain-text tables. Nothing here builds or runs the game.
"""
import argparse
import bisect
import collections
import os
import pickle
import re
import struct
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from owner_map import owner  # noqa: E402

TEXT_LO = 0x12000
SHARED = {"math", "crt", "data", "func"}
SERVICE = SHARED | {"coll", "anim", "cache", "model"}
RAY = {0x244980, 0x2DF720, 0x301800, 0x3009E0}
POSE = {0xBD090, 0xBD970, 0xBDB60, 0xBDC40, 0xBDEF0, 0xB8D30, 0xB8CA0, 0xBA160, 0x109050}
TYPE_TABLE = 0x468630  # object type descriptors, engine-map.md
TYPE_BASES = (0x4678E8, 0x4679B0)  # object, unit
HAVOK_STAGES = [
    ("ray cast (float MOPP)", RAY),
    ("MOPP box query", {0x2DF640, 0x300150, 0x2FF8B0, 0x2FFF90, 0x1EEA50}),
    ("solver and integrate", {0x31C100, 0x31C940, 0x31F620, 0x31BDF0, 0x316A20}),
    ("BV-tree and weld agent", {0x2FA2F0, 0x2FA64E, 0x2FAA60, 0x302EB0, 0x2FA0D0, 0x2FD260, 0x2FD8A0}),
    ("convex agents", {0x2DAF70, 0x2EE610, 0x2EE8B0, 0x3048A0, 0x308590, 0x303360, 0x2DA4D0,
                       0x2E96A0, 0x2E9360, 0x2DBE10, 0x2E0350}),
    ("broadphase and collide driver", {0x3175E0, 0x317950, 0x317D30}),
    ("structure shape callbacks", {0x1EDD00, 0x1EDF50, 0x1EE350, 0x1EF3E0, 0x1EF950, 0x1EFF40}),
    ("character physics", {0x1F19C0, 0x1F2250, 0x1F0450, 0x1E55D0, 0x1F0870, 0x1F1EE0, 0x1F2A80,
                           0x1E5BB0, 0x1E6120}),
]
# The roots of sim-budget.md's three-call census, to show where its
# "Unreached" and "Multiple" rows go.
OLD_GROUPS = {
    "Physics / collision": "1c3d30 1697c0 1de630 14a280 2fa2f0 2ee610 2e96a0 300150 301800",
    "Objects / units": "b6f10 b7060 b7150 bc470 bd090 c6b30 b8d30 bb050 108d90",
    "Animation": "1c6f60 1dd9d0 2798a0 28c470 28c530 28c9e0 1236f0",
    "Players / game mode / weapons": "14d3a0 14d8b0 15c150 15ca10 104150 d9640 1870a0",
    "Effects / particles / lights": "c1400 17a8a0 247140 249170 176e40 173ba0",
    "Sound": "125b40 127400 1293f0 2195f0 221a70",
    "Camera / first person": "1554b0 16f280 23c110 165dc1",
    "AI / scripts": "1c83c0 209310",
    "HUD / UI": "1479c3 147af1",
    "Network / sim replication": "8df50 8dfc0 682e0",
    "Render": "223240 2234e0 2b5d0 2b790",
}
OLD_EXTRA = {0xBC470: [0x108C60], 0x108C60: [0xC6B30], 0xC6B30: [0xC60C0], 0xBDB60: [0x109050],
             0x109050: [0x112070], 0xBAB40: [0x108D90], 0x108D90: [0xBCC20]}
PHASE = {"anim": "1 anim+pose", "model": "1 anim+pose", "fp": "1 anim+pose",
         "coll": "2 collision+queries", "sound": "4 sound", "fx": "6 effects",
         "havok": "5 havok", "havok_glue": "5 havok", "net": "8 net", "ai": "9 ai+hs", "hs": "9 ai+hs"}
for _f in ("obj", "unit", "biped", "veh", "weap", "dev", "dmg", "light", "func"):
    PHASE[_f] = "3 objects+types"
for _f in ("players", "camera", "ui", "input", "gameeng"):
    PHASE[_f] = "7 seats+rules"
OTHER_PHASE = "x cache, main loop, render"


# ---- the image ----------------------------------------------------------

class Image:
    def __init__(self, path):
        self.xbe = open(path, "rb").read()
        base, = struct.unpack_from("<I", self.xbe, 0x104)
        count, table = struct.unpack_from("<II", self.xbe, 0x11C)
        self.sections = []
        for i in range(count):
            at = table - base + i * 0x38
            _flags, va, vsize, raw, rsize, name = struct.unpack_from("<IIIIII", self.xbe, at)
            text = self.xbe[name - base:name - base + 16].split(b"\0")[0].decode()
            self.sections.append((text, va, vsize, raw, rsize))

    def u32(self, va):
        for _n, v, _vs, raw, rs in self.sections:
            if v <= va < v + rs - 3:
                return struct.unpack_from("<I", self.xbe, raw + va - v)[0]
        return None

    def write_elf(self, out):
        """A scratch ELF32 holding .text at its guest address, for objdump."""
        name, va, vsize, raw, rsize = self.sections[0]
        assert name == ".text" and va == TEXT_LO
        body = self.xbe[raw:raw + rsize].ljust(vsize, b"\0")[:vsize]
        names = b"\0.text\0.shstrtab\0"
        off = 0x34
        sh = b"\0" * 40
        sh += struct.pack("<IIIIIIIIII", 1, 1, 6, va, off, len(body), 0, 0, 16, 0)
        sh += struct.pack("<IIIIIIIIII", 7, 3, 0, 0, off + len(body), len(names), 0, 0, 1, 0)
        eh = b"\x7fELF\x01\x01\x01" + b"\0" * 9 + struct.pack(
            "<HHIIIIIHHHHHH", 1, 3, 1, 0, 0, off + len(body) + len(names), 0, 0x34, 0, 0, 40, 3, 2)
        with open(out, "wb") as f:
            f.write(eh + body + names + sh)
        return va, va + vsize


INSN = re.compile(r"^\s*([0-9a-f]+):\s+(\S+)\s*(.*)$")
TARGET = re.compile(r"0x([0-9a-f]+) <")


def disassemble(objdump, elf, lo=None, hi=None):
    cmd = [objdump, "-d", "--no-show-raw-insn", "--x86-asm-syntax=intel", "-j", ".text"]
    if lo is not None:
        cmd += ["--start-address=%d" % lo, "--stop-address=%d" % hi]
    out = subprocess.run(cmd + [elf], capture_output=True, text=True, check=True).stdout
    insns = {}
    for line in out.split("\n"):
        m = INSN.match(line)
        if m:
            a = int(m.group(1), 16)
            if lo is None or lo <= a < hi:
                insns[a] = (m.group(2), m.group(3))
    return insns


class Graph:
    """Static edges of .text: direct calls, jumps that leave a function,
    indirect call sites, and code pointers held in data."""

    def __init__(self, image, functions, work, objdump):
        cache = os.path.join(work, "graph.pkl")
        if os.path.exists(cache):
            self.__dict__.update(pickle.load(open(cache, "rb")))
            return
        elf = os.path.join(work, "text.elf")
        lo, hi = image.write_elf(elf)
        self.starts = sorted(a for a in functions if lo <= a < hi)
        insns = disassemble(objdump, elf)
        # A linear sweep loses its place after data in code (jump tables).
        # Decode again from every function start it missed.
        for b in [s for s in self.starts if s not in insns]:
            i = bisect.bisect_right(self.starts, b)
            end = self.starts[i] if i < len(self.starts) else hi
            for a in [a for a in range(b - 15, end) if a in insns]:
                del insns[a]
            insns.update(disassemble(objdump, elf, b, end))
        self.calls = collections.defaultdict(collections.Counter)
        self.jumps = collections.defaultdict(collections.Counter)
        self.icalls = collections.defaultdict(list)
        self.call_sites = collections.Counter()
        for a in sorted(insns):
            mn, op = insns[a]
            f = self.function(a)
            if mn == "call":
                t = TARGET.match(op)
                if t:
                    self.calls[f][int(t.group(1), 16)] += 1
                    self.call_sites[int(t.group(1), 16)] += 1
                else:
                    self.icalls[f].append(op)
            elif mn.startswith("j"):
                t = TARGET.match(op)
                if t and self.function(int(t.group(1), 16)) != f:
                    self.jumps[f][int(t.group(1), 16)] += 1
        known = set(self.starts)
        self.taken = set()
        for name, va, _vs, raw, rsize in image.sections:
            if name in (".rdata", ".data"):
                for o in range(0, rsize - 3, 4):
                    x = struct.unpack_from("<I", image.xbe, raw + o)[0]
                    if x in known:
                        self.taken.add(x)
        self.type_callbacks = set()
        tables = [image.u32(TYPE_TABLE + 4 * i) for i in range(13)] + list(TYPE_BASES)
        for d in tables:
            for off in range(8, 0x90, 4):
                x = image.u32(d + off) if d else None
                if x in known:
                    self.type_callbacks.add(x)
        self.calls, self.jumps, self.icalls = dict(self.calls), dict(self.jumps), dict(self.icalls)
        pickle.dump(self.__dict__, open(cache, "wb"))

    def function(self, a):
        i = bisect.bisect_right(self.starts, a) - 1
        return self.starts[i] if i >= 0 else None

    def out_edges(self, f):
        return list(self.calls.get(f, {})) + list(self.jumps.get(f, {}))


# ---- the profile --------------------------------------------------------

def category(name):
    """crates/tools/perf/src/symbolize.rs `Category::of_symbol`."""
    if name.startswith(("guest 0x", "@0x")):
        return "guest"
    n = name.lstrip("<&_ ")
    n = n[4:] if n.startswith("mut ") else n
    if n.startswith("h2fn_"):
        return "guest"
    if "fallback::" in n or n.startswith("ir::interp"):
        return "fallback"
    if n.startswith("kernel::"):
        return "kernel"
    if n.startswith("d3d::"):
        return "d3d"
    if n.startswith(("gfx::", "wgpu", "naga::", "metal::", "objc")) or "MTL" in n or "AGX" in n:
        return "gfx"
    if n.startswith(("runtime::", "h2abi_", "h2shim_", "h2x87", "x87::", "simd::", "h2bridge", "h2boot_")):
        return "runtime"
    if n.startswith(("xapi::", "dsound::", "bink_hle::", "audio::", "wma::", "movies::")):
        return "hle"
    return None


def load_profile(path):
    stacks = []
    for line in open(os.path.join(path, "profile.folded")):
        text, n = line.rstrip("\n").rsplit(" ", 1)
        frames = text.split(";")
        cat, at = "other", 0
        for i in range(len(frames) - 1, -1, -1):
            c = category(frames[i])
            if c:
                cat, at = c, i
                break
        guest = [int(f[6:], 16) for f in frames if f.startswith("guest ")]
        stacks.append((frames, int(n), cat, at, guest))
    return stacks


def kind(frames, cat):
    """What a sample with a guest frame was doing."""
    leaf = frames[-1]
    if cat == "guest":
        if leaf.startswith(("guest", "@")):
            return "own"
        if leaf.startswith("h2_direct"):
            return "direct"
        if leaf.startswith(("h2sn_", "h2s_", "h2i_")):
            return "sse"
        return "mem"
    if cat == "runtime":
        return "x87" if "x87" in " ".join(frames[-4:]) else "abi"
    return cat


def host_class(frames):
    """A sample with no guest frame."""
    j = " ".join(frames)
    if "bots::" in j or "bot_glue" in j or "sim_glue::Metered" in j:
        return "bots brain (Rust, inside a hook)"
    if "dsound_glue::advance" in j or "advance_audio" in j:
        return "audio clock advance"
    if "sync_input" in j or "service_input" in j or "InputManager" in j:
        return "XAPI input sync"
    if "sim_glue" in j or "sim::" in j or "telemetry" in j:
        return "sim metrics and telemetry"
    if "file_call" in j or "vfs::" in j:
        return "kernel file I/O"
    if "dsound::" in j or "audio::" in j:
        return "DirectSound HLE calls"
    if "xapi::" in j or "d3d::" in j:
        return "other HLE calls"
    if "hooks::" in j:
        return "other feature hooks"
    if "kernel::Kernel" in j or "kernel::Core" in j or "install_inner" in j:
        return "kernel export calls"
    if "run_on_dpc_stack" in j and "run_from" not in j:
        return "runner: scheduler"
    for f in reversed(frames):
        if f.startswith(("<runtime::", "runtime::", "<kernel::threads")):
            return "runner: " + re.sub(r"[<>]", "", f)[:44]
    return "other host"


class Attribution:
    def __init__(self, graph, stacks):
        self.g, self.stacks = graph, stacks
        self.parents = collections.defaultdict(collections.Counter)
        self.incl = collections.Counter()
        for _f, n, _c, _a, guest in stacks:
            for i, a in enumerate(guest):
                if i:
                    self.parents[a][guest[i - 1]] += n
            for a in set(guest):
                self.incl[a] += n
        self.callers = collections.defaultdict(set)
        for f in list(graph.calls) + list(graph.jumps):
            for t in graph.out_edges(f):
                self.callers[t].add(f)
        self.memo = {}

    def _parents(self, f):
        seen = collections.Counter({q: n for q, n in self.parents.get(f, {}).items() if q != f})
        if sum(seen.values()) >= 3:
            return seen
        if f in self.g.type_callbacks:
            return collections.Counter({"obj": 1})
        return collections.Counter({q: max(1, self.incl[q]) for q in self.callers.get(f, ()) if q != f})

    def _dist(self, f, through, depth, seen):
        if isinstance(f, str):
            return {f: 1.0}
        o = owner(f)[0]
        if o not in through:
            return {o: 1.0}
        if depth > 10 or f in seen:
            return {"?": 1.0}
        p = self._parents(f)
        if not p:
            return {("?" if o in SHARED else "self"): 1.0}
        total, out = sum(p.values()), collections.Counter()
        for q, n in p.items():
            for k, v in self._dist(q, through, depth + 1, seen + (f,)).items():
                out[k] += v * n / total
        return out

    def driver(self, guest, through):
        """The nearest family outside `through` that called into this
        stack. A visible ancestor decides; when the trampoline hid them,
        the function's own observed callers do, then its static callers."""
        for a in reversed(guest):
            if owner(a)[0] not in through:
                return {owner(a)[0]: 1.0}
        key = (guest[0], tuple(sorted(through)))
        if key not in self.memo:
            self.memo[key] = self._dist(guest[0], through, 0, ())
        return self.memo[key]

    def phase(self, guest):
        def one(f):
            o = owner(f)[0]
            if o == "unit" and 0x10DC70 <= f < 0x115000:
                return "1 anim+pose"
            if o == "obj" and f in POSE:
                return "1 anim+pose"
            if f == 0xB89B0:
                return "2 collision+queries"
            return PHASE.get(o, OTHER_PHASE)
        o = owner(guest[-1])[0]
        ray = any(a in RAY for a in guest)
        if o in ("havok", "havok_glue") and ray:
            return {"2 collision+queries": 1.0}
        if o not in SHARED:
            return {one(guest[-1]): 1.0}
        for a in reversed(guest):
            if owner(a)[0] not in SHARED:
                if owner(a)[0] in ("havok", "havok_glue") and ray:
                    return {"2 collision+queries": 1.0}
                return {one(a): 1.0}
        out = collections.Counter()
        for k, v in self.driver(guest, SHARED).items():
            out[PHASE.get(k, OTHER_PHASE)] += v
        return out


KINDS = ["own", "mem", "direct", "sse", "x87", "abi"]


def print_rows(title, rows, runner, total, ms):
    print("\n== " + title)
    print("%-28s %6s %6s %6s %6s %6s %6s %6s | %6s %6s %6s" % (
        "", "own", "mem", "direct", "sse", "x87", "abi", "runner", "all", "%thr", "ms"))
    acq_total = sum(r["acq"] for r in rows.values()) or 1
    for name, row in sorted(rows.items(), key=lambda x: -sum(x[1][k] for k in KINDS)):
        share = runner * row["acq"] / acq_total
        allin = sum(row[k] for k in KINDS) + share
        print("%-28s %6.0f %6.0f %6.0f %6.0f %6.0f %6.0f %6.0f | %6.0f %6.2f %6.3f" % (
            name, *[row[k] for k in KINDS], share, allin, 100 * allin / total, ms * allin / total))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--xbe", required=True)
    ap.add_argument("--functions", required=True)
    ap.add_argument("--profile", required=True)
    ap.add_argument("--work", required=True)
    ap.add_argument("--objdump", default="llvm-objdump")
    ap.add_argument("--ms", type=float, default=0.0)
    ap.add_argument("--log")
    args = ap.parse_args()
    os.makedirs(args.work, exist_ok=True)
    functions = [int(l.split()[0], 16) for l in open(args.functions) if l.startswith("0x")]
    graph = Graph(Image(args.xbe), functions, args.work, args.objdump)
    stacks = load_profile(args.profile)
    at = Attribution(graph, stacks)
    total = sum(n for _f, n, *_ in stacks)
    ms = args.ms or 100.0

    cats = collections.Counter()
    host = collections.Counter()
    by = {"owner": collections.defaultdict(collections.Counter),
          "primary": collections.defaultdict(collections.Counter),
          "phase": collections.defaultdict(collections.Counter)}
    consumer = collections.Counter()
    via = collections.defaultdict(collections.Counter)
    havok = collections.Counter()
    havok_driver = collections.defaultdict(collections.Counter)
    entries = collections.Counter()
    self_fn = collections.Counter()
    conf = collections.Counter()
    hooks = set()
    if args.log:
        for line in open(args.log, errors="replace"):
            m = re.match(r"feature plan: 0x([0-9a-f]+) ", line)
            if m:
                hooks.add(int(m.group(1), 16))

    for frames, n, cat, _at, guest in stacks:
        cats[cat] += n
        if not guest:
            host[host_class(frames)] += n
            continue
        leaf = guest[-1]
        o, c = owner(leaf)
        k = kind(frames, cat)
        k = k if k in KINDS else "abi"
        acquire = frames[-1] == "h2abi_v1_fast_acquire"
        if cat == "guest":
            self_fn[leaf] += n
            conf[c] += n
        splits = {"owner": {o: 1.0}, "primary": at.driver(guest, SHARED), "phase": at.phase(guest)}
        for which, d in splits.items():
            for name, v in d.items():
                by[which][name][k] += v * n
                if acquire:
                    by[which][name]["acq"] += v * n
        through = set(SERVICE)
        if o in ("havok", "havok_glue") and any(a in RAY for a in guest):
            through |= {"havok", "havok_glue"}
        for name, v in at.driver(guest, through).items():
            consumer[name] += v * n
            via[name][o] += v * n
        if o in ("havok", "havok_glue"):
            stage = next((s for a in reversed(guest) for s, members in HAVOK_STAGES if a in members),
                         "other " + o)
            havok[stage] += n
            for name, v in at.driver(guest, SHARED | {"havok", "havok_glue", "coll"}).items():
                havok_driver[stage][name] += v * n
        if acquire:
            sites = graph.icalls.get(leaf, [])
            if leaf in hooks:
                why = "a hooked point, entered from its Rust hook"
            elif leaf in graph.taken and not graph.call_sites.get(leaf):
                why = "indirect entry: address-taken, no direct caller"
            elif any("[0x" not in s or "+" in s or "*" in s for s in sites):
                why = "resume in a function that makes indirect calls"
            elif any(t in hooks for t in graph.calls.get(leaf, {})):
                why = "resume in a function that calls a hooked point"
            else:
                why = "resume after a deeper unwind, a preemption or a refused direct call"
            entries[why] += n

    print("samples %d" % total)
    print("categories: " + ", ".join("%s %d (%.2f%%)" % (c, n, 100 * n / total) for c, n in cats.most_common()))
    runner = sum(n for k, n in host.items() if k.startswith("runner"))
    print("\n== samples with no guest frame")
    for k, n in host.most_common():
        print("%6d %5.2f%%  %s" % (n, 100 * n / total, k))
    print("runner and scheduler together: %d (%.2f%%)" % (runner, 100 * runner / total))
    print_rows("by owning code family", by["owner"], runner, total, ms)
    print_rows("by primary family (math, CRT, datum and function helpers charged to their caller)",
               by["primary"], runner, total, ms)
    print_rows("by rewrite phase", by["phase"], runner, total, ms)

    print("\n== projected saving per phase, ms/tick (own code k times faster; emulation, ABI and")
    print("   runner removed; 40% of x87 kept; 10% of the phase kept as boundary cost)")
    acq_total = sum(r["acq"] for r in by["phase"].values()) or 1
    for name, row in sorted(by["phase"].items()):
        share = runner * row["acq"] / acq_total
        allin = sum(row[k] for k in KINDS) + share
        out = []
        for k in (2, 3, 4):
            saved = (row["mem"] + row["direct"] + row["sse"] + row["abi"] + share
                     + row["own"] * (1 - 1 / k) + 0.6 * row["x87"] - 0.10 * allin)
            out.append("%.2f" % (ms * saved / total))
        print("%-28s all-in %.3f  saves %s" % (name, ms * allin / total, " / ".join(out)))

    print("\n== by end consumer (collision, animation, cache and model lookups charged to who asked)")
    for name, n in consumer.most_common():
        print("%-11s %6.0f %5.2f%%  via %s" % (name, n, 100 * n / total, ", ".join(
            "%s %.0f" % x for x in via[name].most_common(6))))

    print("\n== Havok and its glue by stage")
    for stage, n in havok.most_common():
        print("%-32s %5d %5.2f%%  asked by %s" % (stage, n, 100 * n / total, ", ".join(
            "%s %.0f" % x for x in havok_driver[stage].most_common(5))))

    print("\n== runner entries by cause (samples in h2abi_v1_fast_acquire, one per runner entry)")
    for why, n in entries.most_common():
        print("%5d %5.1f%%  %s" % (n, 100 * n / max(1, sum(entries.values())), why))

    print("\n== confidence of the owner map over guest self samples")
    print(", ".join("%s %d (%.1f%%)" % (c, conf[c], 100 * conf[c] / max(1, sum(conf.values()))) for c in "HML"))

    # Where sim-budget.md's three-call census left its Unreached and Multiple rows.
    roots = {int(x, 16): g for g, s in OLD_GROUPS.items() for x in s.split()}
    reach = collections.defaultdict(set)
    for r, group in roots.items():
        frontier, seen = {r}, {r}
        for depth in range(4):
            for f in frontier:
                reach[f].add(group)
            nxt = set()
            if depth < 3:
                for f in frontier:
                    for t in graph.out_edges(f) + OLD_EXTRA.get(f, []):
                        if t not in seen and not (t in roots and roots[t] != group):
                            seen.add(t)
                            nxt.add(t)
            frontier = nxt
    old = {"Unreached": collections.Counter(), "Multiple": collections.Counter()}
    for f, n in self_fn.items():
        groups = reach.get(f, ())
        if len(groups) != 1:
            old["Unreached" if not groups else "Multiple"][owner(f)[0]] += n
    for name, c in old.items():
        print("\n== three-call census %s: %d self samples, now owned by" % (name, sum(c.values())))
        print(", ".join("%s %d" % x for x in c.most_common()))


if __name__ == "__main__":
    main()
