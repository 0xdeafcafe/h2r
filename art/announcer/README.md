# Announcer lines for our medals

One WAV per medal of our own (docs/game/superbounce.md, "Medals";
docs/game/teabag.md), named by medal id: `superbounce`, `double-bounce`,
`triple-bounce`, `quad-bounce`, `chain-bounce`, `bouncetacular`,
`monster-bounce` and `incel`.
`super-ounce` is an alternate Superbounce take. Halo 2's own announcer lines
are not here; the game plays those from the disc.

**Placeholders.** These lines are AI-generated, not recordings of any
person or from any Halo game, and they will be replaced. This is a personal,
non-commercial fan project. Each is 44.1 kHz mono 16-bit PCM, its
silence trimmed, then matched to the retail announcer's mix by
`scripts/announcer-mix.py`: the median loudness (-14.5 LUFS integrated, peaks
under -0.5 dBTP) and the median balance of five bands around 2 kHz of the
English multi-kill and spree lines, measured from the owner's disc at run time
(`h2 sound shared 'sound\dialog\multiplayer\flavor\<line>' --out DIR`). Nothing
from the disc is stored here.

Run it as `python3 -I scripts/announcer-mix.py RETAIL_DIR SOURCE_DIR OUT_DIR`,
with RETAIL_DIR the exported retail lines and SOURCE_DIR the trimmed,
loudness-levelled sources.

Stored in Git LFS (`.gitattributes`).
