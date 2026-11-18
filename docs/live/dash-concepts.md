# Three directions for h2 Live

**Use Orbital for the player dashboard and Switchboard for Server / Admin.** Orbital makes the menu itself memorable. Move focus from Friends to Messages and the monitor beside the orb changes from a presence list to an inbox. Switchboard spends that same space on player traffic and match history. Silverline is the strongest choice for a short menu opened from inside a game.

## Compare the pages

From this worktree, run `cd web && bun run dev`. Open any of the following pages. The concept selector in the footer keeps the current screen when you switch designs.

| Direction | Start here | Look at |
| --- | --- | --- |
| Orbital | [/dash/concepts/orbital](http://localhost:5174/dash/concepts/orbital) | Move down the fan. Open Friends, choose a contact, then back out. |
| Silverline | [/dash/concepts/silverline](http://localhost:5174/dash/concepts/silverline) | Compare the amber selection with the green blades. Open Account, edit the gamertag, and save. |
| Switchboard | [/dash/concepts/switchboard](http://localhost:5174/dash/concepts/switchboard) | Read the chart alongside the six most recent matches. Choose **Show values** for the half-hour samples. |

Each route has Friends, Messages, Sessions, Account and Server / Admin. To share one directly, append `?screen=friends`, `messages`, `sessions`, `account` or `server` as the screen query value. Northline, the contacts, traffic and game history are fixtures. A saved Live ID belongs to the open preview and resets on reload.

Use the mouse, Tab, arrow keys or a standard-mapped controller. Enter or A selects. B, Escape or Backspace returns to the menu and restores focus to the destination you left. Inside a text field Backspace edits and Escape leaves. The shoulder buttons or Page Up / Page Down switch concepts on the same screen.

## Orbital

Reference 25 gives the home screen its geometry. A luminous core anchors one side, with five small glass sockets following its circumference. Each socket terminates in a blade. A graduated yellow-green face and a slight shift make the selected blade legible across a room. Inside the core, intersecting translucent membranes, a gridded globe and a three-vane relay mark catch the light at different depths.

References 22–24 supply the smaller interactions. A cut-corner plate carries the heading and a wired monitor gives the focused item context. Opening a list or form trades the fan for rows and a detail panel. On a phone the monitor drops below the fan and the detail screens give their width to the controls.

**Adoption cost is mainly in layout and focus.** Keep the fan to a handful of top-level destinations. Put long rosters, clans and settings in the list-and-detail layout. Extract the orb, the socket blade and the preview monitor, then connect their counts and contents to the existing dashboard state. Test long labels, empty lists and focus restoration after live presence changes. The orb is SVG. The animated canvas tunnel is the main recurring drawing cost.

## Silverline

References 20 and 21 arrange a whole conversation inside one piece of hardware. Silverline carries over the undulating silver rim, recessed green field, orange title rule and gold focus. The left half is for action. The right half holds an original sunset relay illustration and the message of the day. Three small counters stay beside the menu. Changing the focused blade leaves the bulletin in place.

**Adoption cost is mainly in the surround.** A shared frame can hold a lobby menu, a single account form or a bulletin equally well. Its top and bottom castings have their own heights so the chrome keeps its curvature around a taller phone screen. Wide tables use more of the interior and leave less space for the silhouette. A real MOTD also needs defined behavior for long copy and a missing illustration. The frame and bulletin are SVG and the focus transition is CSS.

## Switchboard

References 23, 26 and 27 separate the header, status, selected item and detail readout into connected pieces. Switchboard applies that arrangement to server operations. Capacity, lobby count and latency occupy three shallow panels. Below them the traffic chart and history table share a vertical reading edge. The side rail holds the online roster on the overview and gives access to the five destinations. It becomes a small grid on phones.

**Adoption cost is mainly in the data controls.** Keep the deck work on `admin-stats` as the implementation path for real usage data. Its `StatTile`, `TimeChart`, `SeriesTable` and `DenseTable` already cover the next steps, including chart/table switching and sorting. Apply the clipped panel border, socket header and quieter tunnel around those controls. Paginate long histories and update charts at the sampling interval. Drawing more wire detail behind a table buys little.

## Materials, motion and access

The concepts use original SVG geometry, CSS and the existing procedural canvas tunnel. The public art pack already contains raster orbs, blades, frames, prompt glyphs and emblems. All visible concept artwork is drawn in code, including the controller prompts and the MOTD landscape. The reference images remain outside the repository.

Orbitron supplies the squared display lettering. Barlow Condensed carries list labels, descriptions and dense figures. Both ship in the dashboard under the SIL Open Font License 1.1. [Orbitron license](../../web/src/dash/fonts/Orbitron-OFL.txt) · [Barlow Condensed license](../../web/src/dash/fonts/BarlowCondensed-OFL.txt)

The tunnel drifts slowly, the glass pulses, and a glint travels along the cable. Reduced motion holds the tunnel still and disables decorative animation and focus transitions. Text stays on opaque panels, dark text sits on the bright focused blade, and presence includes a word as well as a lamp. The chart has a numerical view, the history has table headings, and the account field has a visible label. Before adopting a direction, check those same focus and contrast treatments with production content at browser zoom and over a real television connection.
