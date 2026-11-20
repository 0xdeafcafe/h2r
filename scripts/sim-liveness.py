#!/usr/bin/env python3
"""Register liveness at the object-update entries and their call sites.

    python3 -I scripts/sim-liveness.py                    # the built-in list, markdown
    python3 -I scripts/sim-liveness.py --text             # plain text, with call details
    python3 -I scripts/sim-liveness.py 0xbc470 0xbdef0    # only these entries
    python3 -I scripts/sim-liveness.py --h2 ~/.cargo/bin/h2 --repo ~/src/h2recomp

It shells out to `h2 disasm <addr> <count>` (run with the main checkout as
the working directory, where disc/default.xbe lives), parses the Intel
syntax it prints and walks the control flow. Nothing from the image is
written anywhere: the output is register and flag names per address.

Two questions, both "read before written along some path":

  entry   For a function entry: which integer registers and which
          arithmetic flags the function reads before it writes them. That
          is what the function takes from its caller.
  site    For a call's return address: which registers and flags the
          caller reads after the return before writing them. That is what
          the caller takes from the callee.

Rules of the walk (docs/engine/object-update.md, "Register liveness"):

  - Every conditional branch is followed both ways. A path stops at `ret`,
    at `int3`, at an indirect `jmp`, at a `jmp` out of the function (a
    target below the entry, a known entry, or one past `int3` padding), when
    every register and flag is written, or after --max-path instructions.
    A visited set keyed by (address, written-state) bounds the work.
  - Sub-registers map to their parent with byte precision: writing `al`
    defines only the low byte of `eax`, so a later read of `eax` is still
    a read of the caller's upper bytes.
  - `xor r,r`, `sub r,r` and `sbb r,r` (the last reads CF) count as writes
    of r without a read of r.
  - A `call` is not entered. The walk records "reaches call X with these
    registers unwritten" (what X could take as arguments, beyond what the
    walk proves), then continues after it as if X wrote no register.
    That over-reports live registers after a call, never under-reports.
    Flags are taken as written by every call (no MSVC code reads flags
    across a call; a callee that preserves them would make this wrong).
  - `esp` is always live and is left out of the output.

Limitations: indirect jumps (jump tables) end the path and are counted;
data inside a function that the linear decoder misreads is not detected;
x87 and SSE registers are not tracked (only integer registers and EFLAGS);
a shift by a zero count is taken as writing the flags.
"""

import argparse
import os
import re
import shutil
import subprocess
import sys

REGS = ["eax", "ecx", "edx", "ebx", "esp", "ebp", "esi", "edi"]
# name -> (parent, byte mask): bit 0 low byte, bit 1 second byte, bit 2 upper half
SUB = {}
for r in REGS:
    SUB[r] = (r, 7)
    SUB[r[1:]] = (r, 3)
for x in "abcd":
    SUB[x + "l"] = ("e" + x + "x", 1)
    SUB[x + "h"] = ("e" + x + "x", 2)
FLAGS = ["CF", "PF", "AF", "ZF", "SF", "OF", "DF"]
ARITH = ("CF", "PF", "AF", "ZF", "SF", "OF")

CC_READS = {
    "o": ("OF",), "no": ("OF",), "b": ("CF",), "c": ("CF",), "nae": ("CF",),
    "ae": ("CF",), "nb": ("CF",), "nc": ("CF",), "e": ("ZF",), "z": ("ZF",),
    "ne": ("ZF",), "nz": ("ZF",), "be": ("CF", "ZF"), "na": ("CF", "ZF"),
    "a": ("CF", "ZF"), "nbe": ("CF", "ZF"), "s": ("SF",), "ns": ("SF",),
    "p": ("PF",), "pe": ("PF",), "np": ("PF",), "po": ("PF",),
    "l": ("SF", "OF"), "nge": ("SF", "OF"), "ge": ("SF", "OF"), "nl": ("SF", "OF"),
    "le": ("ZF", "SF", "OF"), "ng": ("ZF", "SF", "OF"),
    "g": ("ZF", "SF", "OF"), "nle": ("ZF", "SF", "OF"),
}

LINE = re.compile(r"^0x([0-9a-fA-F]+)\s+([0-9a-fA-F]+)\s+(.*)$")
REGWORD = re.compile(r"\b(e?[abcd]x|[abcd][lh]|e?[sd]i|e?[sb]p)\b")
TARGET = re.compile(r"^(?:short\s+)?0*([0-9A-F]+)h$")

# The entries of slices 1 to 5 (docs/engine/sim-rewrite-objects.md, section 5
# "Entries taken over") and the callouts that section names. Their direct
# callees are added by the walk.
ENTRIES = {
    0x0B6F10: "OBJECTS_PRE_PHYSICS", 0x0B7060: "OBJECTS_POST_PHYSICS",
    0x0B7150: "OBJECTS_FINALIZE", 0x0BC470: "OBJECT_PRE_UPDATE",
    0x0BC820: "OBJECT_POST_UPDATE", 0x108C60: "type update dispatch",
    0x108CD0: "type post dispatch", 0x109050: "type pose dispatch",
    0x1090D0: "type after-root dispatch", 0x108D90: "OBJECT_FUNCTION_IMPORT",
    0x108A90: "type new dispatch", 0x109140: "type node-matrix dispatch",
    0x0B7360: "wake", 0x0BAF80: "root ancestor", 0x0BBA20: "wake for post",
    0x0BC7B0: "wake children", 0x109580: "early-mover wake",
    0x0BAB40: "OBJECT_FUNCTION_VALUE", 0x0BA8C0: "function evaluate",
    0x0BCC20: "BASE_OBJECT_IMPORT", 0x0BE8E0: "OBJECT_CHANGE_COLORS",
    0x0C6B90: "unit import", 0x0DE3B0: "biped import", 0x0F6D10: "vehicle import",
    0x0FF5F0: "weapon import", 0x0FBE70: "projectile import",
    0x106D70: "device import", 0x11BD60: "machine and crate import",
    0x0BDEF0: "bounds", 0x0BEF30: "attachment refresh", 0x0BE1D0: "node-matrix notify",
    0x0BC5E0: "component to object", 0x0BD090: "OBJECT_NODE_MATRICES",
    0x0C1400: "lights walk", 0x0C0C80: "LIGHT_RECORD_UPDATE", 0x0C0B00: "light record",
    0x0C2D00: "light list", 0x0C3220: "light list", 0x0C3260: "light list",
}
CALLOUTS = {
    0x0B7B40: "object_new", 0x0B83B0: "delete", 0x0B8460: "delete",
    0x0B7430: "transform set", 0x0B7740: "transform set", 0x0FD910: "deferred weapon fire",
    0x0BF380: "object memory collection", 0x109660: "early-mover record",
    0x0A9570: "network transform apply", 0x1CD4E0: "animation", 0x1CED00: "component",
    0x1D3920: "component nodes", 0x1D44F0: "component sphere", 0x0BB050: "OBJECT_SPATIAL_VISIT",
    0x0D5DE0: "OBJECT_DAMAGE_UPDATE", 0x0B87B0: "partition disconnect",
    0x0B8600: "partition connect", 0x0B8C00: "node matrices of", 0x13BB90: "function source",
    0x0BAEB0: "object iterator next", 0x0FD980: "weapon fire triggers",
}


def default_repo():
    try:
        common = subprocess.check_output(
            ["git", "rev-parse", "--path-format=absolute", "--git-common-dir"],
            text=True, stderr=subprocess.DEVNULL,
        ).strip()
        main = os.path.dirname(common)
        if os.path.exists(os.path.join(main, "disc", "default.xbe")):
            return main
    except (OSError, subprocess.CalledProcessError):
        pass
    return os.getcwd()


class Image:
    """Decoded instructions by address, fetched from `h2 disasm` on demand."""

    def __init__(self, h2, repo, chunk):
        self.h2, self.repo, self.chunk = h2, repo, chunk
        self.ins = {}  # addr -> (size, mnemonic, operands)
        self.order = []

    def fetch(self, addr, count=None):
        out = subprocess.run(
            [self.h2, "disasm", hex(addr), str(count or self.chunk)],
            cwd=self.repo, text=True, capture_output=True,
        )
        if out.returncode != 0:
            raise RuntimeError(f"h2 disasm {addr:#x}: {out.stderr.strip()}")
        got = []
        for line in out.stdout.splitlines():
            m = LINE.match(line.strip())
            if not m:
                continue
            a = int(m.group(1), 16)
            text = m.group(3).strip()
            mn, _, ops = text.partition(" ")
            if mn in ("rep", "repe", "repne", "lock", "repz", "repnz"):
                mn2, _, ops = ops.partition(" ")
                mn = mn + " " + mn2
            self.ins.setdefault(a, (len(m.group(2)) // 2, mn, ops.strip()))
            got.append(a)
        return got

    def at(self, addr):
        if addr not in self.ins:
            self.fetch(addr)
        return self.ins.get(addr)

    def linear(self, start, end):
        """Addresses decoded linearly from start up to end (exclusive)."""
        out, a = [], start
        while a < end:
            i = self.at(a)
            if i is None:
                break
            out.append((a, i))
            a += i[0]
        return out


def split_ops(ops):
    parts, depth, cur = [], 0, ""
    for ch in ops:
        if ch == "[":
            depth += 1
        elif ch == "]":
            depth -= 1
        if ch == "," and depth == 0:
            parts.append(cur.strip())
            cur = ""
        else:
            cur += ch
    if cur.strip():
        parts.append(cur.strip())
    return parts


def reg_of(op):
    op = op.strip()
    return SUB.get(op)


def mem_regs(op):
    if "[" not in op:
        return []
    inner = op[op.index("[") + 1: op.rindex("]")]
    return [SUB[r] for r in REGWORD.findall(inner) if r in SUB]


def target_of(op):
    m = TARGET.match(op.strip())
    return int(m.group(1), 16) if m else None


ALU_RW = {"add", "sub", "and", "or", "xor", "adc", "sbb", "shl", "shr", "sar", "sal",
          "rol", "ror", "rcl", "rcr", "neg", "not", "inc", "dec", "bswap", "xadd"}
WRITE_ONLY = {"mov", "movzx", "movsx", "lea", "movd", "cvttss2si", "cvtss2si",
              "cvttsd2si", "cvtsd2si", "pextrw", "pmovmskb", "movmskps", "movmskpd"}
FLAG_ALL = {"add", "sub", "and", "or", "xor", "adc", "sbb", "cmp", "test", "neg",
            "shl", "shr", "sar", "sal", "imul", "mul", "div", "idiv", "comiss",
            "ucomiss", "comisd", "ucomisd", "bsf", "bsr", "xadd", "cmpxchg", "sahf",
            "fcomi", "fcomip", "fucomi", "fucomip", "popfd", "popf"}
FLAG_NO_CF = {"inc", "dec"}


def effects(mn, ops):
    """(reads, writes, flag_reads, flag_writes, kind, target).

    reads/writes are lists of (reg, mask). kind: None, 'jcc', 'jmp', 'call',
    'ret', 'stop', 'ijmp'."""
    p = split_ops(ops)
    reads, writes, fr, fw = [], [], set(), set()
    kind, tgt = None, None
    base = mn.split()[-1]
    for o in p:
        reads += mem_regs(o)
    if mn.startswith("rep") and len(mn.split()) > 1:
        reads.append(("ecx", 7)); writes.append(("ecx", 7))
    is_string = base in ("stosd", "stosb", "stosw", "lodsd", "lodsb", "lodsw", "movsb",
                         "movsw", "cmpsb", "cmpsw", "scasb", "scasw", "scasd") or (
        base in ("movsd", "cmpsd") and all(o.startswith("[") or "ptr [" in o for o in p))
    if is_string:
        fr.add("DF")
        width = {"b": 1, "w": 3}.get(base[-1], 7)
        if base.startswith("stos"):
            reads += [("edi", 7), ("eax", width)]; writes.append(("edi", 7))
        elif base.startswith("lods"):
            reads.append(("esi", 7)); writes += [("esi", 7), ("eax", 7)]
        elif base.startswith("movs"):
            reads += [("esi", 7), ("edi", 7)]; writes += [("esi", 7), ("edi", 7)]
        else:
            reads += [("esi", 7), ("edi", 7), ("eax", 7)]
            writes += [("esi", 7), ("edi", 7)]
            fw.update(ARITH)
        return reads, writes, fr, fw, None, None

    if base == "int3" or base == "hlt" or base == "ud2":
        return reads, writes, fr, fw, "stop", None
    if base in ("ret", "retn"):
        return reads + [("esp", 7)], writes, fr, fw, "ret", None
    if base == "jmp":
        t = target_of(p[0]) if p else None
        if t is None:
            r = reg_of(p[0]) if p else None
            if r:
                reads.append(r)
            return reads, writes, fr, fw, "ijmp", None
        return reads, writes, fr, fw, "jmp", t
    if base == "call":
        t = target_of(p[0]) if p else None
        if t is None:
            r = reg_of(p[0]) if p else None
            if r:
                reads.append(r)
        return reads + [("esp", 7)], writes, fr, set(FLAGS) - {"DF"}, "call", t
    if base.startswith("j") and base[1:] in CC_READS:
        return reads, writes, set(CC_READS[base[1:]]), fw, "jcc", target_of(p[0])
    if base in ("jecxz", "jcxz"):
        return reads + [("ecx", 7)], writes, fr, fw, "jcc", target_of(p[0])
    if base in ("loop", "loope", "loopne"):
        if base != "loop":
            fr.add("ZF")
        return reads + [("ecx", 7)], writes + [("ecx", 7)], fr, fw, "jcc", target_of(p[0])
    if base.startswith("set") and base[3:] in CC_READS:
        r = reg_of(p[0])
        if r:
            writes.append(r)
        return reads, writes, set(CC_READS[base[3:]]), fw, None, None
    if base.startswith("cmov") and base[4:] in CC_READS:
        d, s = reg_of(p[0]), reg_of(p[1])
        if d:
            reads.append(d); writes.append(d)
        if s:
            reads.append(s)
        return reads, writes, set(CC_READS[base[4:]]), fw, None, None

    regs = [reg_of(o) for o in p]
    if base == "push":
        if regs and regs[0]:
            reads.append(regs[0])
        return reads + [("esp", 7)], writes + [("esp", 7)], fr, fw, None, None
    if base == "pop":
        if regs and regs[0]:
            writes.append(regs[0])
        return reads + [("esp", 7)], writes + [("esp", 7)], fr, fw, None, None
    if base in ("pushfd", "pushf"):
        return reads + [("esp", 7)], writes + [("esp", 7)], set(FLAGS), fw, None, None
    if base in ("popfd", "popf"):
        return reads + [("esp", 7)], writes + [("esp", 7)], fr, set(FLAGS), None, None
    if base in ("pushad", "pusha"):
        return reads + [(r, 7) for r in REGS], writes + [("esp", 7)], fr, fw, None, None
    if base in ("popad", "popa"):
        return reads + [("esp", 7)], writes + [(r, 7) for r in REGS], fr, fw, None, None
    if base == "leave":
        return reads + [("ebp", 7)], writes + [("esp", 7), ("ebp", 7)], fr, fw, None, None
    if base in ("nop", "fnop", "wait", "fwait", "pause", "prefetchnta", "prefetcht0"):
        return reads, writes, fr, fw, None, None
    if base in ("cld", "std"):
        return reads, writes, fr, {"DF"}, None, None
    if base in ("clc", "stc"):
        return reads, writes, fr, {"CF"}, None, None
    if base == "cmc":
        return reads, writes, {"CF"}, {"CF"}, None, None
    if base == "lahf":
        return reads, writes + [("eax", 2)], set(ARITH) - {"OF"}, fw, None, None
    if base == "sahf":
        return reads + [("eax", 2)], writes, fr, set(ARITH) - {"OF"}, None, None
    if base in ("cdq", "cltd"):
        return reads + [("eax", 7)], writes + [("edx", 7)], fr, fw, None, None
    if base in ("cwde", "cwtl"):
        return reads + [("eax", 3)], writes + [("eax", 7)], fr, fw, None, None
    if base == "cbw":
        return reads + [("eax", 1)], writes + [("eax", 3)], fr, fw, None, None
    if base in ("fnstsw", "fstsw") and p and p[0] == "ax":
        return reads, writes + [("eax", 3)], fr, fw, None, None
    if base in ("mul", "imul", "div", "idiv") and len(p) == 1:
        r = regs[0]
        width = r[1] if r else (1 if "byte ptr" in p[0] else 7)
        if r:
            reads.append(r)
        if width == 1:
            if base in ("div", "idiv"):
                reads.append(("eax", 3))
            else:
                reads.append(("eax", 1))
            writes.append(("eax", 3))
        else:
            reads.append(("eax", 7))
            if base in ("div", "idiv"):
                reads.append(("edx", 7))
            writes += [("eax", 7), ("edx", 7)]
        return reads, writes, fr, set(ARITH), None, None
    if base == "imul" and len(p) == 3:
        if regs[1]:
            reads.append(regs[1])
        if regs[0]:
            writes.append(regs[0])
        return reads, writes, fr, set(ARITH), None, None
    if base == "xchg":
        for r in regs:
            if r:
                reads.append(r); writes.append(r)
        return reads, writes, fr, fw, None, None
    if base == "cmpxchg":
        reads.append(("eax", 7)); writes.append(("eax", 7))
        for r in regs:
            if r:
                reads.append(r)
        if regs and regs[0]:
            writes.append(regs[0])
        return reads, writes, fr, set(ARITH), None, None

    if base in ("cmp", "test", "bt"):
        for r in regs:
            if r:
                reads.append(r)
        return reads, writes, fr, ({"CF"} if base == "bt" else set(ARITH)), None, None
    if base in ("bts", "btr", "btc"):
        for r in regs:
            if r:
                reads.append(r)
        if regs and regs[0]:
            writes.append(regs[0])
        return reads, writes, fr, {"CF"}, None, None

    if base in WRITE_ONLY or base in ("movss", "movaps", "movups", "movq", "movdqa",
                                      "cvtsi2ss", "cvtsi2sd", "movlps", "movhps"):
        # mov reg,reg with the same register is padding.
        if len(p) == 2 and regs[0] and regs[1] and regs[0] == regs[1] and base == "mov":
            return [], [], fr, fw, None, None
        if base == "lea" and regs[0] and mem_regs(p[1]) == [regs[0]] and re.fullmatch(
                r"\[\s*" + re.escape(p[0]) + r"\s*(\+\s*0h?)?\]", p[1]):
            return [], [], fr, fw, None, None
        if len(p) >= 2 and regs[1]:
            reads.append(regs[1])
        if p and regs[0]:
            writes.append(regs[0])
        return reads, writes, fr, fw, None, None

    if base in ALU_RW or base in ("shld", "shrd"):
        d = regs[0] if regs else None
        s = regs[1] if len(regs) > 1 else None
        same = d and s and d == s and len(p) == 2
        if same and base in ("xor", "sub"):
            writes.append(d)
            return reads, writes, fr, set(ARITH), None, None
        if d and len(p) == 2 and ((base == "or" and p[1].upper() in ("0FFFFFFFFH", "-1"))
                                  or (base == "and" and p[1] in ("0", "0h"))):
            writes.append(d)
            return reads, writes, fr, set(ARITH), None, None
        if same and base == "sbb":
            writes.append(d)
            return reads, writes, {"CF"}, set(ARITH), None, None
        if d:
            reads.append(d); writes.append(d)
        for r in regs[1:]:
            if r:
                reads.append(r)
        if base in ("shl", "shr", "sar", "sal", "rol", "ror", "rcl", "rcr", "shld", "shrd") \
                and len(p) == 1:
            pass
        if base in ("adc", "sbb", "rcl", "rcr"):
            fr.add("CF")
        if base in FLAG_NO_CF:
            fw = set(ARITH) - {"CF"}
        elif base in ("not", "bswap"):
            fw = set()
        elif base in ("rol", "ror", "rcl", "rcr"):
            fw = {"CF", "OF"}
        else:
            fw = set(ARITH)
        return reads, writes, fr, fw, None, None

    # Everything else (SSE, x87, cpuid...): integer registers only through
    # memory operands; the flag writers we know of.
    if base in FLAG_ALL:
        fw = set(ARITH)
    if base == "cpuid":
        reads.append(("eax", 7)); reads.append(("ecx", 7))
        writes += [(r, 7) for r in ("eax", "ebx", "ecx", "edx")]
    if base == "rdtsc":
        writes += [("eax", 7), ("edx", 7)]
    for o in p[1:]:
        r = reg_of(o)
        if r:
            reads.append(r)
    if p and reg_of(p[0]) and base not in ("movd",):
        # an instruction with an integer destination we do not model: read and write
        r = reg_of(p[0])
        reads.append(r); writes.append(r)
    return reads, writes, fr, fw, None, None


class Result:
    def __init__(self):
        self.live = {}       # reg -> mask read before written
        self.flags = set()
        self.saved = set()   # pushed before being written: saved for the caller, not used
        self.calls = {}      # target (int or 'indirect@addr') -> set of fully unwritten regs
        self.rets = set()    # regs unwritten at some ret
        self.tails = {}
        self.ijmps = set()
        self.truncated = 0
        self.sites = []      # (call addr, return addr, target)
        self.steps = 0


def is_save(img, addr, reg):
    """Is `push reg` (reg not yet written) a save of the caller's value
    rather than an argument? Look ahead on the straight-line code: a save
    sees every part of reg it later reads written first, and no call
    before reg is fully written."""
    done = 0
    for _ in range(24):
        ins = img.at(addr)
        if ins is None:
            return True
        size, mn, ops = ins
        reads, writes, _, _, kind, _ = effects(mn, ops)
        if mn == "push" and reg_of(ops) and reg_of(ops)[0] == reg and done != 7:
            return False
        if any(r == reg and m & ~done for r, m in reads):
            return False
        if kind == "call":
            return done == 7
        for r, m in writes:
            if r == reg:
                done |= m
        if done == 7:
            return True
        if kind is not None:
            return True
        addr += size
    return True


_RET_POP = {}


def ret_pop(img, f, known):
    """Bytes a callee pops on return (`ret N`), or None when its returns
    disagree or none was found."""
    if f in _RET_POP:
        return _RET_POP[f]
    pops, seen, stack = set(), set(), [f]
    while stack and len(seen) < 20000:
        a = stack.pop()
        if a in seen:
            continue
        seen.add(a)
        ins = img.at(a)
        if ins is None:
            continue
        size, mn, ops = ins
        _, _, _, _, kind, tgt = effects(mn, ops)
        if kind == "ret":
            pops.add(int(ops.rstrip("h"), 16) if ops else 0)
            continue
        if kind in ("stop", "ijmp"):
            continue
        if kind == "jmp":
            if tgt is None or tgt < f or (tgt in known and tgt != f) or tgt - f > 0x8000 or any(
                    i[1] == "int3" for _, i in img.linear(f, tgt)):
                continue
            stack.append(tgt)
            continue
        if kind == "jcc" and tgt is not None:
            stack.append(tgt)
        stack.append(a + size)
    _RET_POP[f] = pops.pop() if len(pops) == 1 else None
    return _RET_POP[f]


def esp_delta(img, mn, ops, kind, tgt, known, args=0):
    """Change of esp by one instruction, None when not known. An indirect
    call is taken to pop the `args` dwords pushed since the last call
    (stdcall, as the type callbacks are): the pushes in the run of push,
    mov and lea instructions that ends at the call."""
    p = split_ops(ops)
    base = mn.split()[-1]
    if base in ("push", "pushfd", "pushf"):
        return -4
    if base in ("pop", "popfd", "popf"):
        return 4
    if base in ("pushad", "pusha"):
        return -32
    if base in ("popad", "popa"):
        return 32
    if kind == "call":
        n = ret_pop(img, tgt, known) if tgt is not None else 4 * args
        return None if n is None else n
    if len(p) == 2 and p[0] == "esp":
        m = re.fullmatch(r"(0[0-9A-F]*|[1-9][0-9A-F]*)h?", p[1])
        if base in ("sub", "add") and m:
            v = int(p[1].rstrip("h"), 16)
            return -v if base == "sub" else v
        if base == "lea" and re.fullmatch(r"\[esp\]", p[1]):
            return 0
        return None
    if base == "leave":
        return None
    return 0


def walk(img, start, limit, known, max_states=60000):
    """Read-before-write from `start`. Returns a Result.

    A `push` of a register not yet written is a save, not a read, when the
    same slot is popped back into the same register on some path (esp is
    tracked through pushes, pops, `sub`/`add esp,imm` and the callees'
    `ret N`). Where esp is lost (an indirect call, a frame pointer), the
    straight-line look-ahead in is_save decides."""
    res = Result()
    seen = set()
    sites = {}
    pushes = {}        # push addr -> register
    confirmed = set()  # push addrs popped back into their register
    padding_cache = {}

    def out_of_function(t):
        if t < start or t in known and t != start:
            return True
        if t - start > 0x8000:
            return True
        if t not in padding_cache:
            lin = img.linear(start, t)
            padding_cache[t] = any(i[1] == "int3" for _, i in lin)
        return padding_cache[t]

    # state: addr, defined masks, defined flags, esp offset (None: lost),
    # saved slots ((offset, reg, push addr), ...), depth, live reads made by
    # the pushes on this path (kept out of res.live until classified)
    stack = [(start, tuple(0 for _ in REGS), frozenset(), 0, (), 0, 0)]
    while stack:
        addr, dm, df, esp, slots, args, depth = stack.pop()
        key = (addr, dm, df, esp, slots, args)
        if key in seen:
            continue
        seen.add(key)
        res.steps += 1
        if res.steps > max_states:
            res.truncated += 1
            break
        if depth >= limit:
            res.truncated += 1
            continue
        ins = img.at(addr)
        if ins is None:
            res.truncated += 1
            continue
        size, mn, ops = ins
        reads, writes, fr, fw, kind, tgt = effects(mn, ops)
        defs = dict(zip(REGS, dm))
        pr = reg_of(ops) if mn in ("push", "pop") else None
        new_slots = slots
        # pushes in the run of push/mov/lea that ends at a call: its arguments
        if mn == "push":
            nargs = args + 1
        elif mn.startswith("mov") or mn == "lea":
            nargs = args
        else:
            nargs = 0
        if mn == "push" and pr and pr[0] != "esp" and defs[pr[0]] == 0:
            pushes[addr] = pr[0]
            reads = [x for x in reads if x[0] != pr[0]]
            if esp is not None:
                new_slots = slots + ((esp - 4, pr[0], addr),)
        if mn == "pop" and pr and esp is not None:
            for off, r, pa in slots:
                if off == esp and r == pr[0]:
                    confirmed.add(pa)
        for r, m in reads:
            miss = m & ~defs[r]
            if miss:
                res.live[r] = res.live.get(r, 0) | miss
        for f in fr:
            if f not in df:
                res.flags.add(f)
        unwritten = {r for r in REGS if defs[r] != 7 and r != "esp"}
        if kind == "call":
            name = tgt if tgt is not None else f"indirect at {addr:#x}"
            res.calls.setdefault(name, set()).update(unwritten)
            sites[addr] = (addr + size, tgt)
        for r, m in writes:
            defs[r] |= m
        d = esp_delta(img, mn, ops, kind, tgt, known, args)
        nesp = None if esp is None or d is None else esp + d
        if nesp is None:
            new_slots = ()
        else:
            new_slots = tuple(x for x in new_slots if x[0] >= nesp)
        df2 = df | frozenset(fw)
        ndm = tuple(defs[r] for r in REGS)
        nxt = addr + size
        done = not new_slots and all(defs[r] == 7 for r in REGS if r != "esp") and all(
            f in df2 for f in FLAGS if f != "DF") and ("DF" in df2 or "DF" in res.flags)
        if kind == "stop":
            continue
        if kind == "ret":
            res.rets.update(r for r in REGS if defs[r] != 7 and r != "esp")
            continue
        if kind == "ijmp":
            res.ijmps.add(addr)
            continue
        if kind == "jmp":
            if out_of_function(tgt):
                res.tails.setdefault(tgt, set()).update(
                    r for r in REGS if defs[r] != 7 and r != "esp")
                continue
            if not done:
                stack.append((tgt, ndm, df2, nesp, new_slots, nargs, depth + 1))
            continue
        if done:
            continue
        if kind == "jcc":
            if tgt is not None:
                stack.append((tgt, ndm, df2, nesp, new_slots, nargs, depth + 1))
        stack.append((nxt, ndm, df2, nesp, new_slots, nargs, depth + 1))
    for pa, r in pushes.items():
        size = img.at(pa)[0]
        if pa in confirmed or is_save(img, pa + size, r):
            res.saved.add(r)
        else:
            res.live[r] = 7
    res.sites = sorted((a, r, t) for a, (r, t) in sites.items())
    return res


def all_sites(img, start, known):
    """Every call in the function body: a walk that never stops early."""
    sites, seen, stack = {}, set(), [start]
    while stack:
        a = stack.pop()
        if a in seen or len(seen) > 20000:
            continue
        seen.add(a)
        ins = img.at(a)
        if ins is None:
            continue
        size, mn, ops = ins
        _, _, _, _, kind, tgt = effects(mn, ops)
        if kind in ("stop", "ret", "ijmp"):
            continue
        if kind == "call":
            sites[a] = (a + size, tgt)
        if kind == "jmp":
            if tgt is None or tgt < start or (tgt in known and tgt != start):
                continue
            lin = img.linear(start, tgt) if tgt - start <= 0x8000 else None
            if lin is None or any(i[1] == "int3" for _, i in lin):
                continue
            stack.append(tgt)
            continue
        if kind == "jcc" and tgt is not None:
            stack.append(tgt)
        stack.append(a + size)
    return sorted((a, r, t) for a, (r, t) in sites.items())


def fmt_regs(live):
    out = []
    for r in REGS:
        if r == "esp" or r not in live:
            continue
        m = live[r]
        if m == 7:
            out.append(r)
        elif r[1] in "abcd":
            x = r[1]
            low = {1: x + "l", 2: x + "h", 3: x + "x"}.get(m & 3)
            parts = ([low] if low else []) + ([r + "[31:16]"] if m & 4 else [])
            out.append("+".join(parts))
        else:
            low = r[1:] if m & 3 else None
            out.append("+".join(([low] if low else []) + ([r + "[31:16]"] if m & 4 else [])))
    return " ".join(out) or "none"


def fmt_set(s):
    return " ".join(r for r in REGS if r in s) or "none"


def fmt_flags(f):
    return " ".join(x for x in FLAGS if x in f) or "none"


def name_of(a, names):
    return f"{a:x}" + (f" {names[a]}" if a in names else "")


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("entries", nargs="*", help="entry addresses (hex); default: the built-in list")
    ap.add_argument("--h2", default=shutil.which("h2") or os.path.expanduser("~/.cargo/bin/h2"))
    ap.add_argument("--repo", default=None, help="checkout holding disc/default.xbe")
    ap.add_argument("--max-path", type=int, default=400)
    ap.add_argument("--chunk", type=int, default=400, help="instructions per h2 disasm call")
    ap.add_argument("--text", action="store_true", help="plain text with call details")
    ap.add_argument("--no-callees", action="store_true", help="do not add direct callees")
    ap.add_argument("--no-sites", action="store_true")
    a = ap.parse_args()
    img = Image(a.h2, a.repo or default_repo(), a.chunk)
    names = dict(CALLOUTS)
    names.update(ENTRIES)
    known = set(names)
    if a.entries:
        roots = [int(x, 16) for x in a.entries]
    else:
        roots = list(ENTRIES)
    callees = []
    site_rows = []
    for e in roots:
        if a.no_sites:
            continue
        for call, ret, tgt in all_sites(img, e, known):
            r = walk(img, ret, a.max_path, known)
            site_rows.append((e, call, ret, tgt, r))
            if tgt is not None and tgt not in roots and tgt not in callees:
                callees.append(tgt)
    entry_list = list(roots)
    if not a.entries:
        entry_list += [c for c in CALLOUTS if c not in entry_list]
    if not a.no_callees:
        entry_list += [c for c in callees if c not in entry_list]
    entry_rows = [(e, walk(img, e, a.max_path, known)) for e in entry_list]

    if a.text:
        for e, r in entry_rows:
            print(f"entry {name_of(e, names)}: reads {fmt_regs(r.live)}; flags {fmt_flags(r.flags)}"
                  f"; saved {fmt_set(r.saved)}"
                  f"; ret with unwritten {fmt_set(r.rets)}; ijmp {len(r.ijmps)}; truncated {r.truncated}")
            for t, s in sorted(r.calls.items(), key=lambda x: str(x[0])):
                tn = name_of(t, names) if isinstance(t, int) else t
                print(f"    reaches call {tn} with unwritten {fmt_set(s)}")
            for t, s in sorted(r.tails.items()):
                print(f"    tail jmp {name_of(t, names)} with unwritten {fmt_set(s)}")
        for e, call, ret, tgt, r in site_rows:
            tn = name_of(tgt, names) if tgt is not None else "indirect"
            print(f"site in {e:x}: call {call:x} -> {tn}, returns to {ret:x}: reads "
                  f"{fmt_regs(r.live)}; flags {fmt_flags(r.flags)}; to ret unwritten {fmt_set(r.rets)}"
                  f"; ijmp {len(r.ijmps)}; truncated {r.truncated}")
        return

    print("| Entry | Reads before writing | Flags read | Saved and restored | Reaches `ret` with unwritten | Notes |")
    print("|---|---|---|---|---|---|")
    for e, r in entry_rows:
        notes = []
        if r.ijmps:
            notes.append(f"{len(r.ijmps)} indirect jmp")
        if r.truncated:
            notes.append("truncated")
        if r.tails:
            notes.append("tail " + ", ".join(f"`{t:x}`" for t in sorted(r.tails)))
        print(f"| `{e:x}`{' ' + names[e] if e in names else ''} | {fmt_regs(r.live)} | "
              f"{fmt_flags(r.flags)} | {fmt_set(r.saved)} | {fmt_set(r.rets)} | {'; '.join(notes)} |")
    if site_rows:
        print()
        print("| In | Call | Returns to | Callee | After return, reads | Flags | Reaches `ret` with unwritten |")
        print("|---|---|---|---|---|---|---|")
        for e, call, ret, tgt, r in site_rows:
            tn = f"`{tgt:x}`" if tgt is not None else "indirect"
            note = " (truncated)" if r.truncated else ""
            note += f" ({len(r.ijmps)} indirect jmp)" if r.ijmps else ""
            print(f"| `{e:x}` | `{call:x}` | `{ret:x}` | {tn} | {fmt_regs(r.live)}{note} | "
                  f"{fmt_flags(r.flags)} | {fmt_set(r.rets)} |")


if __name__ == "__main__":
    sys.exit(main())
