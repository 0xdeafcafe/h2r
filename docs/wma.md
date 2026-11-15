# ASF and retail WMAv2

`crates/asf` demuxes the retail sound permutations. `crates/wma` decodes them
natively to interleaved i16 stereo PCM. `h2 sound` exports these permutations as
WAV, including sounds tagged Xbox ADPCM with `Encoding::Codec`.
**Measured against the game's own decoder (Unicorn, 48 streams): within 2 LSB
everywhere, within 1 LSB on all but 4 of about 27 million samples, 101.7 to
105.6 dB SNR, lengths equal.** It is not bit-exact; see "Oracle results".

## Source boundary

Inputs to this implementation are Microsoft's public ASF specification and the
owner's retail XBE/streams. No FFmpeg, Symphonia, WMA fork, or other WMA decoder
implementation was read. The earlier private media research's suggestion to use
FFmpeg as a reference is superseded: only the game's decoder is the oracle.
No game bytes, extracted tables, streams, disassembly listings, or PCM belong in
this repository. Tests use synthetic structures and analytically generated
signals, not reduced or transformed game fixtures.

The ASF specification was read from a [mirror of Microsoft's June 2010 document][spec]
and cross-checked with [Microsoft's ASF structure overview][overview]. Codec
behavior below was recovered using `h2 disasm` (the repository's XBE reader and
iced-x86 disassembler). Algorithm descriptions and address identifiers below are
provenance facts, not a reproduction of machine code or table contents.

## Provenance ledger

**S** is public specification, **E** is the retail XBE, **V** is private corpus
validation, **Design** is an implementation choice. Addresses are virtual, not
file offsets. `Tables::load` translates them through the XBE's section map.

| Structure or operation | Source | Evidence |
|---|---|---|
| GUID/size framing, header count, file properties | ASF revision 01.20.05, June 2010, §§2, 3.1–3.2, 10 | S |
| Stream properties, WAVEFORMATEX | Same specification §§3.3, 9.1 | S |
| Header extension objects | Same specification §3.4; unknown bodies preserved | S |
| Data object and packet count | Same specification §5.1 | S |
| Packet flags, error correction, padding | Same specification §§5.2.1–5.2.2 | S |
| Fragments, replicated data, compressed subpayloads | Same specification §5.2.3 | S |
| Audio spread, encryption identifiers | Same specification §§9.1.1, 10 | S |
| Chunk concatenation | `Gestalt::read_encoded`; see `sound.md` | Existing V |
| Bounded fragment assembly and stream API | Independently written | Design |
| WMAv2 WAVEFORMATEX mapping | `0x38903d..0x389072`; creator call `0x388ac1..0x388b01` | E |
| Profile, maximum frame length, variable block limit | `0x38b086`, `0x38caf0..0x38cd2d` | E |
| MSB reader, superframe sequence/count/reservoir offset | `0x38ce6e..0x38d15e`, `0x38a4e3`; offset width `0x38cd40..0x38cd8f` | E |
| Packet reset and frame accounting | `0x38a6bd..0x38a850`; zero-frame packets are legal | E, V |
| Block-size history and frame boundaries | `0x38aaef`, `0x38fb92..0x38fd24` | E |
| Channel flags, mid/side, total gain, exponent refresh | `0x38fd24..0x390113` | E |
| Huffman relative-node layout and extended leaves | `0x392447..0x392509` | E |
| Run/level symbols, sign, escape, EOB | `0x38f46b..0x38f5bf`; escape width `0x38c069` | E |
| Gain and exponent normalization | `0x38c0d5`, `0x38c146`, `0x38c180` | E |
| Right-channel mid/side codebook selection | `0x38f63f` | E |
| Noise enable decision | `0x390507..0x390648` | E; disabled for both accepted profiles |
| Frequency cutoff and band reuse across block sizes | `0x38ccbc`, `0x38bdfe`, `0x38c180` | E |
| Mid/side sum and difference | `0x38ad32..0x38ad9d` | E |
| Transform pre/post rotations and scaling | `0x3913cd..0x391653` | E; independent cosine-definition test |
| Complex FFT | `0x3911d1` / `0x390ef1`; native implementation uses ordinary radix-2 butterflies | E for sign/order; Design for implementation |
| Window selection and transition extents | `0x3321d5..0x332367`, `0x38bdfe..0x38c069` | E |
| Folded overlap, intermediate signed 17-bit range, output saturation | `0x38d3c3`, `0x38ea85` | E |
| Overlap tail negation past 3n/2 | `fchs` at `0x38d918` in the second save loop (`0x38d8ec..`); the first loop (`0x38d43c..`) has none | E, V |
| Initial silent-frame suppression | `0x38c440`, `0x38c773`, `0x38ca58` | E |

## Runtime table catalogue

`VerifiedImage::load` requires whole-image SHA-256
`03215919bb7163259257d361f4c7bf802a7ab12aa85e2689436369b5c427935d`
**before** parsing/address lookup. This authenticates every table and immediate
operand together. A different image fails rather than supplying guessed tables.
Values are copied into typed vectors only at runtime. The digest is an identity,
not redistributed game content.

| Data | XBE address / extent | Consumer / extraction |
|---|---|---|
| Exponent Huffman tree | `0x3929b8..0x392b80` | `0x390019`; u16 relative nodes |
| Family-3 ordinary coefficient tree | `0x392c18..0x3932e8` | `0x38af67`; u16 nodes |
| Ordinary runs / levels | `0x39b898..0x39bc50` / `0x39bc50..0x39c008` | u16 arrays, indexed by symbol minus two |
| Family-3 mid/side right tree | `0x394928..0x394fc8` | `0x38f65e`; u16 nodes |
| Mid/side right runs / levels | `0x39c008..0x39c370` / `0x39c370..0x39c6d8` | u16 arrays |
| Critical frequencies | `0x39b67c`, 25 u16 | `0x38ba6a`; long-block cumulative bands rounded in groups of four |
| 512-coefficient band boundaries | Immediate operands in `0x38b63c..0x38b6b2` | Read operands at runtime; group-of-four rounding `0x38b6c5` |
| 256-coefficient band boundaries | Stores in `0x38b733..0x38b787`, shared stores `0x38b8fb..0x38b90f` | Operand addresses in `tables.rs`; no copied values |
| 128-coefficient band boundaries | Stores `0x38b7a6..0x38b7f5`, push immediate at `0x38b79d` | Operand extraction, then same rounding |
| Exponent-ratio correction | `0x39b248`, 72 f32 | `0x38c146`; combined with a power-of-two divisor |
| Gain fraction / powers | `0x39b3e8`, 146 f32 / `0x39b630`, 19 f32 | `0x38c0d5` |
| Small/large gain constants | `0x4172cc`, `0x4172d0`, `0x45dc1c` f32; `0x45e150` f64 | Same gain routine |
| Signed windows, sizes 2048/1024/512/256/128 | `0x3971c8`, `0x3991c8`, `0x39a1c8`, `0x39a9c8`, `0x39adc8`; size f32 each | Pointer assignments `0x38c727`, selector `0x3321d5`; first half negative |
| Rotation records | Pointer array `0x39b200`, index block-size/128; 11 f32 per selected record | `0x391420..0x391462`; pre/post recurrence seeds |
| Profile decision constants | `0x45e2fc`, `0x4172e0`, `0x4172d4` f32 | Stereo bitrate adjustment, noise threshold, family-3 threshold |
| Band arithmetic constants | `0x45dbc0`, `0x45dc0c`, `0x45dc20` f32 | General band construction |

The FFT's twiddle angles are computed at runtime, as the XBE's FFT computes
angles with x87 sine/cosine. No twiddle table is compiled into the Rust binary.
No noise table is loaded: the recovered decision disables noise substitution at
both accepted bitrates. Lower bitrate profiles, other coefficient families, LSP
exponents, WMAv1, and noise synthesis are explicitly outside this decoder's scope.

## APIs and bounds

`Asf::parse` borrows a complete permutation. `packets()` checks payloads lazily;
`Reassembler` accepts consecutive fragments for one stream and must be finished
at EOF. Unknown objects remain opaque. The finite-file profile rejects broadcast,
variable packet sizes, encryption, packets above 64 KiB, more than 4096
header/extension objects, and more than 4096 expanded payloads per packet.
Multi-packet audio spreading and media objects without declared size are
unsupported. Fragment gaps, overlaps, and abandoned objects fail.

`Tables::load(&VerifiedImage)` is reusable across permutations. `Decoder::new`
accepts only 44.1 kHz stereo WMAv2, byte-rate/block-align pairs 12003/4459 and
16002/5945, with the retail flags. `push` accepts one codec block and returns
interleaved i16 PCM, using one packet of lookahead for the reservoir. `finish`
consumes the final packet; it does not fabricate a missing continuation. Errors
poison the decoder. Bit consumption, tree depth, coefficient runs, block bounds,
and total gain are bounded. Memory per packet is bounded independently of file
duration. `decode_asf` additionally takes an interleaved sample limit (CLI: 128 Mi
interleaved samples) and verifies decoded frame count against superframe headers.

Output retains the codec's final padding; it is not trimmed or stretched to ASF
millisecond duration. The engine's initial suppression is applied. No automatic
alignment, gain fitting, or concealment is used for oracle comparisons.

## Private validation

- ASF: **485/485 unique streams, zero parse/reassembly failures**, 3627 packets,
  17,572,605 codec bytes. Five 128 kbps and 480 96 kbps streams. Deduplication
  uses physical chunk spans across mainmenu and shared, including unreferenced
  permutations. Shared also references the five mainmenu streams.
- Native: **485/485, zero decode failures**, 22,367 codec frames, 46,656 blocks,
  43,823,104 stereo PCM frames. No oracle was run during this native stage.
- Click diagnostic (mean absolute step at codec-frame boundaries vs interior,
  channel 0): 463.794 vs 453.332 after the overlap fix. **This metric cannot
  detect overlap-add errors.** Before the fix it read 463.758 vs 462.029 (ratio
  1.004) and looked clean, yet the output was at about 5 dB SNR. It only samples
  codec-frame boundaries, at multiples of 2048. The overlap bug corrupted the
  second half of each block, with its error decaying to zero at the block's end,
  so the clicks sat at odd multiples of 1024 (mean step about 4670 vs 310 on
  stream 0), where this metric never looks. Use the oracle comparison instead.
- ASF duration residual: 19.3–2076.1 samples. The harness checks against one
  final 2048-sample frame plus 1 ms timestamp quantization (44.1 samples).
- Stream 388 is silent and has zero nonzero decoded coefficients; this is
  internally consistent, and the oracle's output for it is also all zero.
- Spectral diagnostics use three 1024-point Hann windows per channel per stream.
  Maximum fraction above 11.025 kHz: 0.408053; maximum absolute channel mean:
  52.876 i16 units; maximum clipped fraction: 0.005674. These measurements do
  not establish that noisy/clipped content matches the reference.
- `h2 sound mainmenu sound\\ui\\main_menu_music\\menu_music\\in` exported a
  stereo 44.1 kHz PCM WAV successfully. Private output is retained outside Git.
- Synthetic tests cover ASF truncation, malformed sizes, fragment consistency,
  mutation safety, MSB bit reads, Huffman nodes, codec headers, streaming silence,
  error poisoning, transform correctness, and comparison/spectral measurements.
  `overlap_add_cancels_time_domain_aliasing` analyses a synthetic two-tone
  signal with a textbook MDCT over a mixed block-size sequence and requires
  `synthesize` to rebuild it within 2 LSB. Without the tail negation it fails
  with errors of about 22800. Decoder tests cover both retail profiles. They
  also require explicit rejection, followed by poisoning, for a sequence
  discontinuity and for a total gain above 512.

## Oracle handoff

The earlier Unicorn attempt crashed in host cache initialization, before guest
execution. The user authorized continuing native work and assigned execution to
an agent outside the Codex sandbox. **Do not rerun Unicorn in this sandbox.**
The runner has since been run outside the sandbox on 48 streams (see "Oracle
results"). Its guest ABI and EOF driver produce output of the right length.

From this worktree, outside the affected sandbox:

```sh
CARGO_TARGET_DIR="$TMPDIR/target" /opt/homebrew/bin/cargo run -p oracle --features unicorn --example wma_oracle -- /Users/afr/Source/github.com/0xdeafcafe/h2recomp/disc/halo2.iso 5 /Users/afr/.config/rush/keep/h2recomp-b5afe71a/586b38da/oracle-5.pcm
```

Native PCM files for indices **0, 5, 38, and 388 are already prepared** in the
retained directory used above. Skip this native command for the initial handoff.
For reproduction, use the same zero-based index and choose a fresh output path
(the example below refuses to overwrite the prepared file):

```sh
CARGO_TARGET_DIR="$TMPDIR/target" /opt/homebrew/bin/cargo run -p wma --example native_pcm -- /Users/afr/Source/github.com/0xdeafcafe/h2recomp/disc/halo2.iso 5 /Users/afr/.config/rush/keep/h2recomp-b5afe71a/586b38da/native-5.pcm
```

Comparison (raw little-endian signed i16 stereo; no WAV headers):

```sh
CARGO_TARGET_DIR="$TMPDIR/target" /opt/homebrew/bin/cargo run -p wma --example compare -- /Users/afr/.config/rush/keep/h2recomp-b5afe71a/586b38da/native-5.pcm /Users/afr/.config/rush/keep/h2recomp-b5afe71a/586b38da/oracle-5.pcm
```

Output reports both lengths, compared length, max absolute error, RMSE, and SNR.
A length mismatch exits nonzero and labels the common-prefix measurement; it is
never silently trimmed away. Output creation is exclusive. Select a fresh path
for a repeated run. Useful indices: 0 (128 kbps music), 5 (96 kbps, variable
blocks), 38 (includes zero-frame reservoir packet), 388 (native silent output).
`asf::disc::Disc` defines index order: mainmenu gestalt order, then shared,
physical-span deduplication. Both exporters use that same enumerator.

Oracle entries recovered from the XBE:

- Create `0x38a0c5`, eleven stdcall arguments: output handle pointer, input
  context, callback, version 2, extra DWORD, sample rate, channels, byte rate,
  block align, flags, zero. The game's caller is `0x388ac1..0x388b01`.
- Packet begin `0x38a189`, decode `0x38a1a7`, PCM retrieval `0x38a1cc`.
  Container driver `0x388533` handles statuses; the harness drains PCM after
  each decode and begins another packet on status 4.
- Callback ABI `(data**, size*, context, staging_buffer)` at `0x38ce7d..0x38ce8c`;
  128-byte cap `0x388a21`, return 3 at packet boundaries. EOF returns zero bytes.
- Allocator/free `0x322e16` / `0x324c91` use bounded private guest memory.
  Calls have instruction/time budgets. Guest execution is x86 Pentium III;
  x87 control word is explicitly 0x027f and MXCSR 0x1f80.

The harness must set CR0 = 0x21 and CR4 = 0x600 (OSFXSR) before running guest
code. Without them the decoder's SSE `cvttss2si` at `0x3901a2` raises #UD.

## Oracle results

48 streams: 0 to 5 (0 to 4 are 128 kbps), every 12th index from 10 to 478, and
38 and 388. All are stereo. Comparison is raw: no alignment, trimming or gain.

- Lengths are equal on all 48.
- Maximum absolute error is 1 LSB on 44 streams and 0 on silent stream 388.
  Streams 1, 3 and 4 (the long 128 kbps ones) have 1, 2 and 1 samples at
  2 LSB.
- About 0.06% of samples differ at all. SNR is 101.7 to 105.6 dB.
- The residue is float rounding. The native FFT uses a different operation
  order from the game's, and output and overlap are rounded separately, so two
  1-LSB differences can stack.

Before the fix, the same streams gave 3.8 to 6.1 dB, with every other
1024-sample segment wrong. The native fold left out the engine's negation of
the last quarter of the 2n folded output (i >= 3n/2) when saving overlap
(`fchs` at `0x38d918`). That broke time-domain aliasing cancellation in the
second half of every following block.

An ignored test, `crates/oracle/tests/wma_disc.rs`, reruns 128 kbps stream 0
(the shortest of 0 to 4; about 25 s in release) and 96 kbps streams 5, 38, 154 and 226
against the guest decoder and requires at most 1 LSB. It reads the disc
by absolute path:
`cargo test -p oracle --features unicorn --release --test wma_disc -- --ignored`.

Remaining gap: integration into live engine playback (only export is wired).

## Work record

- Done: ASF milestone and 485-stream parse validation.
- Done: native PCM and WAV export (step 1 report delivered).
- Done: internal energy, spectrum, click, and duration checks.
- Done: oracle runner and comparison tool built; executed externally (below).
- Done: 15 ASF/WMA synthetic tests and four CLI sound tests; formatting and
  Clippy with warnings denied for the changed packages (`--no-deps`). The wider
  dependency-lint run hits an existing `xbe` constant-chunk lint under Rust 1.98;
  unrelated dependency source was left unchanged. Oracle-feature Clippy also
  passed (build/lint only, no tests or execution with Unicorn enabled).
- Done: oracle run on 48 streams. Found and fixed the overlap tail sign.
  Native now matches within 1 LSB, apart from 4 samples at 2 LSB.

[spec]: https://exse.eyewated.com/fls/54b3ed95bbfb1a92.pdf
[overview]: https://learn.microsoft.com/en-us/windows/win32/medfound/asf-file-structure
