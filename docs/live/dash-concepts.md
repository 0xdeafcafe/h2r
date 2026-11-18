# Live dashboard

The dashboard now uses a glass orb, a lit connector on the selected menu blade,
and recessed panels with brushed edges. Open **Games → Halo 2** for recent
matches and your signed-in service record. **View stats** opens the Halo 2 stats site.

The Games screen draws the game's title image (the art's `title/image`) on
the Halo 2 row, in the orb and on the panel, and a drawn disc without it.
Map names come from the same place as the stats site's (the server's
`catalog.json`, then the art manifest's Halo 1 names, then the stem), so
they need the disc's art on the server: on the Worker, the published copy.

**Saved games** lists the signed-in player's cloud saves for Halo 2
([live-cloud-saves.md](live-cloud-saves.md)), the way the original
dashboard's Memory screen listed a game's saves: one row per save
container with its picture, the name in its `SaveMeta.xbx`, the date of
its newest file and its size in the original's 16 KiB blocks, then how
much the account's saves take against the server's quota. A save's
picture is its own `SaveImage.xbx`, else the title's shared one on U:,
else the disc's `title/save-image`, else a drawn one. Select a save to
delete it: it asks first (A again to delete, B to keep), then deletes the
container's files one by one against the versions listed, its
`SaveMeta.xbx` last and only when the rest are gone, so a file another
machine changed in the meantime is kept (409) and the save keeps its name.
A machine that has the save deletes it at its next sign-in and keeps a
copy under `deleted-elsewhere` ([live-cloud-saves.md](live-cloud-saves.md#conflicts)); one where it
changed since keeps it. The list and
the deletes use the existing cloud-save endpoints; deleting needs the
page on the server's own origin, like the other writes.
