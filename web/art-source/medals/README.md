# Medal source art

Our modes' medals, the superbounce family and Incel, drawn natively at
512 × 512 (`<id>-512.png`) and halved (`<id>-256.png`), for banners and other
art: not served (outside `web/public/`) and named by no page. Halo 2's own
medals have no source art here. `python3 scripts/live-medals.py --out DIR
--hires` draws Incel and the bounce medals into `DIR/hires/`. Monster Bounce (`monster-bounce`) is drawn by
`python3 scripts/superbounce-medals.py --rs FILE --art DIR --only monster-bounce`,
also into `DIR/hires/`. See docs/live/live-art.md, "Stats banners".
