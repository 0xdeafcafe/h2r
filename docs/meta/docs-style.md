# How docs here are written

Readers are coding agents first and the owner second. A page states the
current design; git has the history. Every sentence should be safe to act on
without checking whether it is still true.

## Kinds of page

| Kind | Where | States | Changes |
|---|---|---|---|
| Design | `docs/<area>/<topic>.md` | how our code works, or an unbuilt design (the status line says which) | in the same branch as the code |
| Reference | `docs/<area>/<topic>.md` | facts that don't move: the retail binary, disc formats, a CLI or API contract | when a new fact is established |
| Guide | `docs/<area>/<topic>.md` | how to do a task: build, deploy, run a check | when the steps change |
| Notes | `docs/<area>/notes/<topic>.md` | dated evidence: measurements, experiments, research | appended; old entries are never rewritten |
| Review | `docs/reviews/<yyyy-mm-dd>-<name>.md` | one dated review | never, after its date |
| Index | `docs/README.md` | every page, one line each | when a page is added or removed |

Areas are the existing folders: `lifter`, `runtime`, `perf`, `engine`,
`formats`, `graphics`, `audio`, `input`, `game`, `halo1`, `live`, `editor`,
`dev`. `docs/reviews` holds reviews and `docs/meta` holds this page.

A design page never holds a logbook and a note never states the design. A
page with both gets split: the design keeps the path, the dated material
moves to `notes/`.

### Notes

A list of dated entries, newest first. Each entry gives the date, the commit,
the machine or method, and how to reproduce it. The page opens with one line
naming the design page it supports. Nothing in `notes/` says what the code
does today.

Each area keeps one `notes/not-adopted.md`: one row per experiment that lost
(what it was, the result, the commit that removed it), so nobody builds it
twice.

### Reviews

Frozen on their date. Follow-up shows in the design pages and the code, not
in the review.

## Writing

Write like an engineer explaining the system to a colleague who has the code
open: short, plain, direct.

- **Lead with the point.** The first sentence under a heading says what the
  section is about. No preamble, no "This section describes".
- **One idea per paragraph**, two to four sentences. If a paragraph needs a
  fifth, it is two ideas or a list.
- **Tables over prose** for anything with rows: fields, methods, switches,
  stages, options, results. Don't follow a table with prose that restates it.
- **Say it once.** No summary at the top that the body repeats, no recap at
  the bottom, no "as described above". Link instead.
- **Let the code say what the code says.** Name the file and symbol; don't
  narrate the function line by line.
- **No narration.** No "We started by", "Then I kept going", "It turned out".
  State the result.
- **No hedging.** A fact is stated. A guess carries the **S** label and what
  would verify it.

## Paths and headings are an interface

Code comments cite docs by path and often by section (`docs/game/maps.md §2`,
`docs/halo1/sky.md, "Indoor fog"`): about 2,500 citations of 157 pages, 680
naming a section.

- Don't rename or move a design, reference or guide page. To split one, keep
  its path and add new files beside it.
- Before removing or renaming a heading, find its citations:
  `git grep -n 'docs/<area>/<page>.md' -- crates platform scripts web`. Keep a
  cited heading's exact text, or fix the comment in the same branch.
- New pages don't number their headings. A numbered page keeps the numbers
  code cites.

## No archive

A page that no longer describes anything is deleted; git keeps it.

- Delete in its own commit: `Delete docs/<path>: <reason>`, body says what
  replaced it.
- `git log --diff-filter=D --name-only -- docs` lists deleted pages;
  `git show <commit>^:docs/<path>` prints one.
- A superseded design is deleted. Reasoning that still binds is a sentence on
  the current page.
- A lost experiment gets its row in `notes/not-adopted.md`; its page goes.
- Never in `docs/`: a lane's contract or handoff, a list of files a lane
  touched, a coordinator checklist, a review of one branch. Those go in the
  report or the commit message.

## House style

1. **Present tense.** "The runner charges one unit per block", not "was
   changed to charge". Write as if the design had always been this way.
2. **No history.** No changelog, "as built" section, phase diary, dated
   heading, "now", "previously", "no longer", "since 2026-11-18". Commit
   hashes appear only in `notes/` and in a "Measured at" line.
3. **Cite the code.** Behaviour names the file or symbol
   (`crates/hooking/hooks/src/switches.rs`, `feature::switches::Switches`).
   "Tested" names the test.
4. **One home per fact.** Switches and environment variables are listed once,
   in `docs/runtime/switches.md`; other pages name the switch and link there.
5. **One status line**, under the title (see the template).
6. **Evidence labels** on anything not plain from the code (below).
7. **Numbers come from notes.** A measured number links to the note with its
   date, commit and method.
8. **Say what is not verified** in a "Not verified" section: plain open items,
   no dates or names.
9. **No game data.** Public repo. Addresses and field offsets appear as named
   research points. No bytes, tables, strings, constants or asset names from
   the disc.

10. **Short.** One topic per page, about 600 lines. A longer page is
    split by topic, not by date: the original keeps a short index and every
    heading that code cites, and the new pages are linked from
    `docs/README.md`.
11. **Lead with a picture.** Each page, and each section that explains a
    flow or a set of states, opens with a table, an ASCII diagram, or a
    mermaid flowchart or sequence diagram, before any prose. States, slices
    and options are tables. Flows are mermaid.
12. **Short prose.** Terse, claim first, at most about 3 sentences a
    paragraph, numbers over adjectives. A longer paragraph is a table or a
    list.
13. **Useful, not a journal.** A page says what the thing is, how it
    works, how to use or change it, and what is open. Cut run logs,
    step-by-step histories, "we tried", repeated caveats, measurements that
    no decision rests on, and restated code. Ask of each paragraph whether
    a new contributor would miss it. Evidence a decision rests on goes in
    `notes/` as one dated table with the commit, linked in one line.
14. **The owner's voice.** British spelling (behaviour, optimise, colour,
    analyse, centre, -isation). No em dashes: a spaced hyphen, commas,
    brackets or a new sentence. None of the banned words (`delve`,
    `leverage` as a verb, `robust`, `seamless`, `unlock`, `harness` as a
    verb, `utilise`, `streamline`, `landscape`, `paradigm`, `synergy`,
    `journey`, `game-changer`, `cutting-edge`, `superpower`, `in essence`,
    `fundamentally`, `essentially`). No manufactured contrasts ("this
    isn't X, it's Y": say Y) and no connective filler (`Furthermore`,
    `Moreover`, `Additionally`, `Indeed`, `It's worth noting`).

## Evidence labels

| Label | Meaning | Give with it |
|---|---|---|
| **E** | read from the retail XBE with `h2 disasm` | the guest address |
| **V** | verified on the owner's disc | the `#[ignore]` test or command |
| **Code** | read from this repository | the path |
| **Test** | shown by a test on synthetic data | the test name |
| **Run** | seen in a running game | the agent API method or command, and the note |
| **S** | supposed, not verified | what would verify it |
| **Design** | our choice, not a fact about the game | nothing |

Status words: **Built**, **Partly built**, **Design** (not built).

A page with its own scheme in an "Evidence labels" section
(`halo1/models.md` has V1 to V5, the sim-rewrite specs have C, L and O) keeps
it when it carries more than this table, and never mixes two schemes. Cite
the path, not "read from this repository at `<commit>`".

## Template for a design page

```markdown
# <What it is>

Status: Built | Partly built | Design. <One sentence: what "partly" leaves out.>
Code: `crates/<crate>/src/<file>.rs` (<entry point>). Tests: `crates/<crate>/tests/<file>.rs`.
Switch: `<name>` (<default>), listed in [switches.md](../runtime/switches.md). Or: none.

<Two to five sentences: what it does, what it sits between, the one rule
that must hold.>

## How it works

<The mechanism in the order the code runs. Name files and symbols.>

## Interfaces

<What calls it and what it calls. For a guest boundary: each entry point
with its address as a named research point, what the guest still reads,
what must still be written.>

## Rules and limits

<Invariants, what it refuses and what happens then, known gaps.>

## Evidence

<Labelled facts not plain from the code, each with its source. Numbers link
to notes/.>

## Not verified

<What only a game run or a disc test can show.>

## Related

<Pages this one depends on, and its notes.>
```

A reference page swaps "How it works" for the layout or contract it records.
A guide is a numbered list of steps with commands. An unbuilt design uses the
same template with `Status: Design`, written in the present tense of the
design ("the collision world owns...").

## Keeping docs current

A change to behaviour, an interface, a switch or a path a doc names updates
that doc in the same branch. The agent rule is "Keep the docs current" in
`AGENTS.md`.

## Checking

`scripts/docs-check.py` reads the tree with git and python3 and runs nothing
from the project.

```sh
scripts/docs-check.py                  # everything
scripts/docs-check.py docs/perf        # only these paths
scripts/docs-check.py --strict docs/x  # soft findings fail too
```

| Check | Finds | Fails the run |
|---|---|---|
| `links` | relative links and `#anchors` that don't resolve | yes |
| `index` | a page `docs/README.md` doesn't link | yes |
| `code` | a `docs/...md` path in a code comment that doesn't exist | yes |
| `code-sections` | a section a code comment cites that is no longer a heading | with `--strict` |
| `names` | an `H2_*` variable or `impl.*` switch no code reads | with `--strict` |
| `switches` | an `H2_*` variable the code reads that `docs/runtime/switches.md` doesn't list (a `H2_FOO_*` row covers the family) | with `--strict` |
| `paths` | a backticked `crates/...` path that doesn't exist | with `--strict` |
| `style` | a heading that records history (a date, "as built", "superseded") | with `--strict` |
| `voice` | an em dash, a banned word, an American spelling, or a connective filler | on a page changed in the branch; a warning elsewhere |
| `length` | a paragraph over 4 sentences | on a page changed in the branch; a warning elsewhere |
| `pagelen` | a page over 600 lines | on a page changed in the branch; a warning elsewhere |

`names`, `paths` and `style` skip `notes/` and `docs/reviews/`. `names` and
`paths` also skip a page whose status line says Design or plan. In a Partly
built page, mark a proposed path with `(new)` or `(planned)` right after it
and `paths` skips it.

Code may cite a heading, a numbered section (`§ 5.2`) or a bold lead-in
(`**Sky.**`, `- **Sky.**`); `code-sections` accepts all three.
