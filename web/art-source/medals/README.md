# Medal source art

The Live and superbounce medals drawn natively at 512 × 512 (`<id>-512.png`)
and halved (`<id>-256.png`), for banners and other art: not served (outside
`web/public/`) and named by no page. `python3 scripts/live-medals.py --out DIR
--hires` draws them into `DIR/hires/`. Monster Bounce (`monster-bounce`) is drawn by
`python3 scripts/superbounce-medals.py --rs FILE --art DIR --only monster-bounce`,
also into `DIR/hires/`. See docs/live/live-art.md, "Stats banners".
