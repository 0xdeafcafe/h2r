# A shared Halo 1 cache: measurements

Evidence for [../shared-map.md](../shared-map.md).

## 2026-11-20: what the converted maps repeat, and the room in the shared caches

Commit: `ebe96cdb0`. Machine: the owner's Mac, the owner's Halo 2 disc
image and the converted maps installed under the data root
(`<data>/maps/h1_*`, `<data>/campaigns/halo1`). Method: the read-only
script at the end of this entry. It reads headers, the tag index and tag
names, and hashes `0x200`-byte sectors; it prints sizes and counts only.

The installed files are from two import runs on the same day: the Halo 1
sandbox files of the multiplayer maps from the morning's, the Halo 2
sandbox files from the evening's (converter version 101 in the stamp).
Differences between a map's two files therefore include whatever the
converter changed in between. The campaign files predate the stamp too.
Repeat the run after a full `h2 import halo1 --force` before relying on
a single map's row.

Sector hashing only sees a repeat when the bytes sit at the same offset
modulo `0x200` in both files, so every "repeated" figure is a lower
bound. All-zero sectors are not counted. "Raw" means the file before the
tag index (header offset `+0x10`): raw resources, BSP sections and
strings, without tag metadata.

### Retail maps on the disc

Limits are `map::writer::cache_size_limit` (file) and
`map::writer::max_metadata_size` (tag memory, header `+0x1c`; the bound
is the largest retail map of the type, so the largest map of each type
shows no room).

| Map | Type | File | Of the type's limit | Room in the file | Tag memory | Under the type's bound |
|---|---|---|---|---|---|---|
| `shared` | shared | 174.9 MiB | 97.2% | 5.1 MiB | `0x445000` | 0 |
| `single_player_shared` | campaign shared | 409.1 MiB | 78.7% | 110.9 MiB | `0x69e000` | 0 |
| `mainmenu` | menu | 56.9 MiB | 71.1% | 23.1 MiB | `0x1d3000` | 0 |
| `ascension` | multiplayer | 28.4 MiB | 35.4% | 51.6 MiB | `0x528000` | `0x104000` |
| `beavercreek` | multiplayer | 27.6 MiB | 34.5% | 52.4 MiB | `0x4d4000` | `0x158000` |
| `burial_mounds` | multiplayer | 30.6 MiB | 38.3% | 49.4 MiB | `0x53d000` | `0xef000` |
| `coagulation` | multiplayer | 39.5 MiB | 49.3% | 40.5 MiB | `0x551000` | `0xdb000` |
| `colossus` | multiplayer | 29.7 MiB | 37.1% | 50.3 MiB | `0x4eb000` | `0x141000` |
| `cyclotron` | multiplayer | 44.1 MiB | 55.1% | 35.9 MiB | `0x521000` | `0x10b000` |
| `foundation` | multiplayer | 35.9 MiB | 44.8% | 44.1 MiB | `0x4fd000` | `0x12f000` |
| `headlong` | multiplayer | 53.5 MiB | 66.9% | 26.5 MiB | `0x5d7000` | `0x55000` |
| `lockout` | multiplayer | 32.9 MiB | 41.2% | 47.1 MiB | `0x4b8000` | `0x174000` |
| `midship` | multiplayer | 29.1 MiB | 36.4% | 50.9 MiB | `0x53d000` | `0xef000` |
| `waterworks` | multiplayer | 39.4 MiB | 49.3% | 40.6 MiB | `0x62c000` | 0 |
| `zanzibar` | multiplayer | 51.1 MiB | 63.8% | 28.9 MiB | `0x60b000` | `0x21000` |

Campaign levels, for the same two columns: files 66.0 to 260.3 MiB of
280 MiB (the least room 19.7 MiB, on `07a_highcharity`); tag memory
`0x565000` to `0xc24000` of `0xc24000` (under `0x80000` of room on
`01b_spacestation`, `03b_newmombasa`, `04b_floodlab`,
`06a_sentinelwalls`, `06b_floodzone`, `07a_highcharity` and
`08b_deltacontrol`).

The header validator's absolute file limit is `0x20800000` (520 MiB,
docs/halo1/sounds.md "Campaign loops and cache slots"): above
`shared.map`'s 174.9 MiB that leaves 345 MiB.

### Converted multiplayer maps: the Halo 1 sandbox against the Halo 2 sandbox

"Tags added" are tags (by group and name) in the Halo 1 sandbox file and
not in the Halo 2 sandbox file. "Raw only in h1" is the raw sectors of
the Halo 1 sandbox file that its Halo 2 sandbox file does not hold.

| Map | File, h1 | File, h2 | Tag memory, h1 | Tag memory, h2 | Tags added | Their metadata | Raw only in h1 |
|---|---|---|---|---|---|---|---|
| `h1_beavercreek` | 32.4 MiB | 18.2 MiB | `0x3f9000` | `0x3db000` | 702 | 0.4 MiB | 15.6 MiB |
| `h1_bloodgulch` | 36.6 MiB | 19.1 MiB | `0x340000` | `0x464000` | 845 | 0.6 MiB | 20.4 MiB |
| `h1_boardingaction` | 35.2 MiB | 21.0 MiB | `0x461000` | `0x443000` | 703 | 0.4 MiB | 16.0 MiB |
| `h1_carousel` | 31.4 MiB | 16.5 MiB | `0x3e7000` | `0x3c4000` | 755 | 0.4 MiB | 16.5 MiB |
| `h1_chillout` | 30.2 MiB | 16.0 MiB | `0x3cc000` | `0x3ae000` | 702 | 0.4 MiB | 15.4 MiB |
| `h1_damnation` | 35.3 MiB | 21.8 MiB | `0x424000` | `0x409000` | 701 | 0.4 MiB | 15.4 MiB |
| `h1_hangemhigh` | 32.3 MiB | 18.1 MiB | `0x3cd000` | `0x3ae000` | 704 | 0.4 MiB | 15.5 MiB |
| `h1_longest` | 30.7 MiB | 16.5 MiB | `0x3c9000` | `0x3ab000` | 701 | 0.4 MiB | 15.5 MiB |
| `h1_prisoner` | 32.4 MiB | 18.1 MiB | `0x3e3000` | `0x3c4000` | 702 | 0.4 MiB | 15.5 MiB |
| `h1_putput` | 30.8 MiB | 15.0 MiB | `0x41f000` | `0x3fe000` | 730 | 0.4 MiB | 17.4 MiB |
| `h1_ratrace` | 35.8 MiB | 21.1 MiB | `0x3d2000` | `0x3b4000` | 722 | 0.4 MiB | 16.4 MiB |
| `h1_sidewinder` | 40.4 MiB | 22.9 MiB | `0x34a000` | `0x46d000` | 846 | 0.6 MiB | 20.5 MiB |
| `h1_wizard` | 29.7 MiB | 15.4 MiB | `0x3cf000` | `0x3b1000` | 703 | 0.4 MiB | 15.6 MiB |
| All 13 | 433.4 MiB | 239.8 MiB | | | | 5.6 MiB | 215.9 MiB |

- Tags added: 690 are in every map, 932 in at least one.
- Of the 215.9 MiB of raw only in the Halo 1 sandbox files, one copy of
  each distinct sector is **28.9 MiB**; the other **187.0 MiB** is
  repeats across maps. By the number of maps holding a distinct sector:
  13 maps 13.5 MiB, 11 or 12 maps 0.4 MiB, 2 to 10 maps 5.7 MiB, one map
  9.3 MiB.
- The 690 tags in every map, by metadata on one map (0.4 MiB in all):
  `bitm` 129 tags 74 KiB, `jmad` 12 tags 55 KiB, `coll` 15 tags 50 KiB,
  `effe` 39 tags 43 KiB, `mode` 28 tags 40 KiB, `shad` 86 tags 27 KiB,
  `prt3` 53 tags 15 KiB, `phmo` 1 tag 15 KiB, `jpt!` 42 tags 14 KiB,
  `proj` 11 tags 13 KiB, `weap` 8 tags 11 KiB, `hlmt` 21 tags 11 KiB,
  `nhdt` 10 tags 10 KiB, `ligh` 20 tags 4 KiB.
- Not in "their metadata": what the sandbox adds to tags both files have
  (the sound gestalt's entries, the scenario's collections), and string
  ids. The two tag memory columns do not give it either: both files are
  built from scratch from their own closure, so the Halo 1 sandbox file
  leaves out Halo 2 sandbox tags the other keeps, and the two columns
  are from different import runs (above; the two vehicle maps show it).

### Converted campaign levels: Halo 1 assets against Halo 2 assets

| Level | File, h1 | File, h2 | Tag memory, h1 | Tag memory, h2 | h1 raw also in its h2 file |
|---|---|---|---|---|---|
| a10 | 198.0 MiB | 195.6 MiB | `0x9bb000` | `0x9ab000` | 170.5 MiB (98%) |
| a30 | 120.5 MiB | 114.5 MiB | `0x9c6000` | `0x995000` | 101.0 MiB (94%) |
| a50 | 173.3 MiB | 169.5 MiB | `0xa35000` | `0xa29000` | 148.2 MiB (97%) |
| b30 | 137.0 MiB | 130.2 MiB | `0x8ef000` | `0x8b3000` | 116.2 MiB (94%) |
| b40 | 226.8 MiB | 215.4 MiB | `0xabd000` | `0xa51000` | 181.5 MiB (94%) |
| c10 | 200.7 MiB | 196.6 MiB | `0x95d000` | `0x94f000` | 162.3 MiB (97%) |
| c20 | 163.3 MiB | 155.8 MiB | `0x819000` | `0x7ee000` | 120.4 MiB (95%) |
| c40 | 229.7 MiB | 216.4 MiB | `0x93b000` | `0x8ca000` | 187.4 MiB (94%) |
| d20 | 179.2 MiB | 176.3 MiB | `0x924000` | `0x90e000` | 154.7 MiB (98%) |
| d40 | 264.6 MiB | 260.0 MiB | `0x9eb000` | `0x9c3000` | 227.3 MiB (98%) |
| All 10 | 1,893.1 MiB | 1,830.2 MiB | | | 1,569.5 MiB |

Across the ten Halo 1 assets levels: 1,632.4 MiB of non-zero raw sectors,
998.3 MiB distinct, so 634.1 MiB is repeats across levels; the distinct
sectors held by two or more levels are 242.9 MiB.

### Import time

`<data>/campaigns/halo1/import-timing.txt` of the evening run (converter
version 101): 13 multiplayer jobs (the Halo 2 sandbox files), 4.9 s wall
on 10 workers, 0.6 to 1.3 s a job, 66.6% of the CPU time in "build map".
The campaign's twenty builds took 260 s in the disc test
([../campaign.md](../campaign.md) §5).

### Reproduce

```sh
mkdir -p "$TMPDIR/h1shared" && $EDITOR "$TMPDIR/h1shared/measure.py"   # the script below
python3 -I "$TMPDIR/h1shared/measure.py" disc/halo2.iso "$HOME/Library/Application Support/h2recomp/data"
```

About 17 s. Nothing it reads or prints is kept.

```python
#!/usr/bin/env python3
"""Read-only measurements for docs/halo1/shared-map.md. Prints sizes and counts only."""
import hashlib, os, struct, sys, collections

SECTOR = 0x200

class Src:
    def __init__(self, path, base=0, size=None):
        self.f = open(path, 'rb'); self.base = base
        self.size = size if size is not None else os.path.getsize(path)
    def read(self, off, n):
        self.f.seek(self.base + off); return self.f.read(n)

def u32(b, o): return struct.unpack_from('<I', b, o)[0]

def parse(src):
    h = src.read(0, 0x800)
    assert h[:4] == b'daeh' and u32(h, 4) == 8, 'not a v8 map'
    I, D, msize, total = u32(h, 0x10), u32(h, 0x14), u32(h, 0x18), u32(h, 0x1c)
    ih = src.read(I, 0x20)
    G, ng, T, nt = u32(ih, 0), u32(ih, 4), u32(ih, 8), u32(ih, 0x18)
    magic = (G - 0x20) - I
    ent = src.read(T - magic, nt * 16)
    ncount, nblob, nlen, noff = u32(h, 0x2c0), u32(h, 0x2c4), u32(h, 0x2c8), u32(h, 0x2cc)
    offs = struct.unpack('<%dI' % ncount, src.read(noff, ncount * 4))
    blob = src.read(nblob, nlen)
    tags = {}
    for i in range(nt):
        cls, datum, addr, size = struct.unpack_from('<4sIII', ent, i * 16)
        if datum == 0xffffffff: continue
        name = b''
        if i < ncount and offs[i] != 0xffffffff:
            e = blob.index(b'\0', offs[i]); name = blob[offs[i]:e]
        tags[(cls[::-1], name)] = size
    return dict(file=u32(h, 8), type=u32(h, 0x140), meta=msize, total=total, D=D,
                slots=nt, tags=tags, meta_off=I + D, index=I)

def sectors(src, lo=0, hi=None):
    hi = src.size if hi is None else hi
    out = collections.Counter(); off = lo
    while off < hi:
        buf = src.read(off, min(1 << 22, hi - off))
        for i in range(0, len(buf) - SECTOR + 1, SECTOR):
            s = buf[i:i + SECTOR]
            if s.count(0) == SECTOR: continue
            out[hashlib.blake2b(s, digest_size=8).digest()] += 1
        off += len(buf)
    return out

def mib(n): return '%.1f MiB' % (n / 1048576)

def xiso_maps(path):
    f = open(path, 'rb'); size = os.path.getsize(path)
    for base in (0, 0x18300000, 0xfd90000, 0x2080000):
        if base + 0x10020 > size: continue
        f.seek(base + 0x10000); v = f.read(0x20)
        if v[:20] == b'MICROSOFT*XBOX*MEDIA': break
    else: raise SystemExit('no XDVDFS volume in ' + path)
    root, rsize = struct.unpack_from('<II', v, 20)
    def walk(sec, dsize, prefix, out):
        f.seek(base + sec * 2048); d = f.read(dsize)
        def node(o):
            if o * 4 >= len(d): return
            l, r, s, sz, attr, nl = struct.unpack_from('<HHIIBB', d, o * 4)
            if l == 0xffff: return
            name = d[o * 4 + 14:o * 4 + 14 + nl].decode('latin1').lower()
            if l: node(l)
            if attr & 0x10:
                if sz: walk(s, sz, prefix + name + '/', out)
            else: out[prefix + name] = (base + s * 2048, sz)
            if r: node(r)
        node(0)
    out = {}; walk(root, rsize, '', out)
    return {k: v for k, v in out.items() if k.endswith('.map')}

LIMIT = {0: 0x11800000, 1: 0x05000000, 2: 0x05000000, 3: 0x0b400000, 4: 0x20800000}
BOUND = {0: 0xc24000, 1: 0x62c000, 2: 0x1d3000, 3: 0x445000, 4: 0x69e000}

def retail(iso):
    print('== retail maps:', os.path.basename(iso))
    print('map | type | file | of limit | file headroom | tag memory +0x1c | under the type bound | slots')
    for name, (off, sz) in sorted(xiso_maps(iso).items()):
        m = parse(Src(iso, off, sz)); t = m['type']
        print('%s | %d | %s | %.1f%% | %s | 0x%x | 0x%x | %d' % (
            os.path.basename(name)[:-4], t, mib(m['file']), 100 * m['file'] / LIMIT[t],
            mib(LIMIT[t] - m['file']), m['total'], BOUND[t] - m['total'], m['slots']))

def group_bytes(tags, keys):
    g = collections.Counter(); n = collections.Counter()
    for k in keys: g[k[0]] += tags[k]; n[k[0]] += 1
    return g, n

def multiplayer(data):
    root = os.path.join(data, 'maps')
    maps = sorted(d for d in os.listdir(root) if d.startswith('h1_'))
    print('\n== converted multiplayer maps (%d), Halo 1 sandbox against Halo 2 sandbox' % len(maps))
    print('map | file h1 | file h2 | tag memory h1 | tag memory h2 | tags added | their metadata | raw sectors only in h1')
    added_sets = []; only = {}; tot = collections.Counter(); per_added = {}
    for d in maps:
        a = Src(os.path.join(root, d, 'h1', d + '.map')); b = Src(os.path.join(root, d, 'h2', d + '.map'))
        ma, mb = parse(a), parse(b)
        added = set(ma['tags']) - set(mb['tags'])
        added_sets.append(added); per_added[d] = (ma['tags'], added)
        sa, sb = sectors(a, 0, ma['index']), sectors(b, 0, mb['index'])
        o = {k: c for k, c in sa.items() if k not in sb}; only[d] = o
        nbytes = sum(o.values()) * SECTOR
        meta = sum(ma['tags'][k] for k in added)
        print('%s | %s | %s | 0x%x | 0x%x | %d | %s | %s' % (
            d, mib(ma['file']), mib(mb['file']), ma['total'], mb['total'], len(added), mib(meta), mib(nbytes)))
        tot['file_h1'] += ma['file']; tot['file_h2'] += mb['file']; tot['only'] += nbytes; tot['meta'] += meta
    common = set.intersection(*added_sets); union = set.union(*added_sets)
    t0, a0 = per_added[maps[0]]
    print('all maps: h1 files %s, h2 files %s, sectors only in h1 %s, added-tag metadata %s' % (
        mib(tot['file_h1']), mib(tot['file_h2']), mib(tot['only']), mib(tot['meta'])))
    print('added tags by (group, name): in every map %d, in any map %d' % (len(common), len(union)))
    seen = collections.Counter()
    for d in maps:
        for k in only[d]: seen[k] += 1
    uniq = len(seen) * SECTOR
    print('h1-only sectors, one copy of each distinct sector: %s (the rest, %s, is repeats across maps)' % (
        mib(uniq), mib(tot['only'] - uniq)))
    hist = collections.Counter(seen.values())
    print('distinct h1-only sectors by number of maps holding them:',
          ', '.join('%d maps: %s' % (k, mib(v * SECTOR)) for k, v in sorted(hist.items())))
    # metadata of the common set, by group, from the first map
    for label, keys in (('in every map', common),):
        tags = per_added[maps[0]][0]
        g, n = group_bytes(tags, keys)
        print('added tags %s, metadata by group (%s): total %s' % (label, maps[0], mib(sum(g.values()))))
        print('  ' + ', '.join('%s %d tags %.0f KiB' % (k.decode('latin1'), n[k], v / 1024) for k, v in g.most_common(14)))

def campaign(data):
    root = os.path.join(data, 'campaigns', 'halo1')
    levels = sorted(f[:-4] for f in os.listdir(root) if f.endswith('.map'))
    print('\n== converted campaign levels (%d), Halo 1 assets against Halo 2 assets' % len(levels))
    print('level | file h1 | file h2 | tag memory h1 | tag memory h2 | h1 sectors also in its h2 file')
    seen = collections.Counter(); tot = collections.Counter()
    for l in levels:
        a = Src(os.path.join(root, l + '.map')); ma = parse(a); sa = sectors(a, 0, ma['index'])
        p2 = os.path.join(root, 'h2', l + '.map'); line = ''
        na = sum(sa.values()) * SECTOR
        if os.path.exists(p2):
            b = Src(p2); mb = parse(b); sb = sectors(b, 0, mb['index'])
            both = sum(c for k, c in sa.items() if k in sb) * SECTOR
            tot['both'] += both; tot['h2'] += mb['file']
            line = '%s | 0x%x | 0x%x | %s (%.0f%%)' % (mib(mb['file']), ma['total'], mb['total'], mib(both), 100 * both / na)
        print('%s | %s | %s' % (l, mib(ma['file']), line))
        tot['h1'] += ma['file']; tot['nz'] += na
        for k in sa: seen[k] += 1
    uniq = len(seen) * SECTOR
    print('all levels: h1 files %s, h2 files %s; h1 non-zero sectors %s, distinct %s, in two or more levels %s' % (
        mib(tot['h1']), mib(tot['h2']), mib(tot['nz']), mib(uniq),
        mib(sum(1 for v in seen.values() if v > 1) * SECTOR)))
    print('sectors of an h1 level also in its own h2 level: %s' % mib(tot['both']))

if __name__ == '__main__':
    iso, data = sys.argv[1], sys.argv[2]
    retail(iso); multiplayer(data); campaign(data)
```
