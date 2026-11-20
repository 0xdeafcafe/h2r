# How docs here are written

The readers are coding agents first and the owner second. A doc states the
current design. Git has the history. An agent that reads a page should be
able to act on every sentence without checking whether it is still true.

[docs-tidy-plan.md](docs-tidy-plan.md) is the one-off plan that brings the
existing docs to these rules. This page is the standing rule.

## Kinds of page

| Kind | Where | What it states | When it changes |
|---|---|---|---|
| Design | `docs/<area>/<topic>.md` | how a part of our code works now, or a design that is not built yet (the status line says which) | in the same branch as the code |
| Reference | `docs/<area>/<topic>.md` | facts that do not move: the retail binary, the disc formats, a CLI or API contract | when a new fact is established |
| Guide | `docs/<area>/<topic>.md` | how to do a task: build a target, deploy, run a check | when the steps change |
| Notes | `docs/<area>/notes/<topic>.md` | dated evidence: measurements, experiment results, research records | appended to; old entries are not rewritten |
| Review | `docs/reviews/<yyyy-mm-dd>-<name>.md` | one dated review of the code or a plan | never, after its date |
| Index | `docs/README.md` | every page, one line each | when a page is added or removed |

Areas are the existing folders: `lifter`, `runtime`, `perf`, `engine`,
`formats`, `graphics`, `audio`, `input`, `game`, `halo1`, `live`, `editor`.
`docs/meta` holds this page.

A design page never holds a logbook, and a note never states the design.
When a page has both, split it: the design stays at the page's path and the
dated material moves to `notes/`.

### Notes

A notes page is a list of dated entries, newest first. Each entry has the
date, the commit it was measured or read at, the machine or method, and how
to reproduce it. A notes page starts with one line naming the design page
it is evidence for. Nothing in `notes/` is a statement of what the code
does today; a reader who wants that follows the link.

Each area keeps one `notes/not-adopted.md`: experiments that were tried and
lost, one row each (what it was, the result, the commit that removed it).
Its purpose is to stop the same experiment being built twice.

### Reviews

A review is frozen on its date. Follow-up does not go back into it: what
was done about a finding shows in the design pages and in the code.

## Paths and headings are an interface

Code comments cite docs by path, and often by section: about 2,500
citations of 157 pages, 680 of them naming a section (`docs/game/maps.md
§2`, `docs/halo1/sky.md, "Indoor fog"`). So:

- Do not rename or move a design, reference or guide page. To split one,
  keep its path for the design and create new files beside it.
- Before removing or renaming a heading, look for citations of it:
  `git grep -n 'docs/<area>/<page>.md' -- crates platform scripts web`.
  Keep a cited heading's title, or fix the citing comment in the same
  branch.
- New pages do not number their headings. A numbered page keeps the
  numbers that code cites.

## No archive

There is no archive folder. A page that no longer describes anything is
deleted, and git keeps it.

- Delete in a commit of its own. The subject is `Delete docs/<path>:
  <reason>`, and the body says what replaced it.
- `git log --diff-filter=D --name-only -- docs` lists every deleted page;
  `git show <commit>^:docs/<path>` prints one.
- A superseded design is deleted, not kept "for the reasoning". If the
  reasoning still binds the current design, it is a sentence on the
  current design page.
- An experiment that lost gets its row in `notes/not-adopted.md` and its
  page is deleted.
- These never go in `docs/`: a lane's contract or handoff, a list of files
  a lane touched, a checklist for the coordinator, a review of one branch.
  They belong in the report or the commit message.

## House style

1. **Present tense, plain statements.** "The runner charges one unit per
   block", not "the runner was changed to charge". Write the page as if the
   design had always been this way.
2. **No history.** No changelog, no "as built" or "what was built"
   section, no phase or slice diary, no date in a heading, no "now",
   "previously", "no longer", "since 2026-11-18", "an earlier version". A
   commit hash appears only in `notes/` and in a "Measured at" line.
3. **Cite the code.** A claim about behaviour names the file or the symbol
   (`crates/feature/src/switches.rs`, `feature::switches::Switches`). A
   claim that something is tested names the test. A reader should be able
   to open the code from the sentence.
4. **One home per fact.** Link to it; do not restate it. Switches and
   environment variables are listed once, in
   `docs/runtime/switches.md`; other pages name the switch and link there.
5. **One status line**, directly under the title (see the template).
6. **Evidence labels** on anything that is not plain from the code (below).
7. **Numbers come from notes.** A measured number on a design page links
   to the note that has its date, commit and method.
8. **Say what is not verified.** A "Not verified" section lists what only a
   game run can show, as plain open items without dates or names.
9. **No game data.** This is a public repo. Addresses and field offsets
   appear as named research points. No bytes, tables, strings, constants or
   asset names from the disc.
10. **Short.** A design page over about 600 lines is two pages. Split by
    topic, not by date.

## Evidence labels

Use these on new and rewritten pages.

| Label | Meaning | What to give with it |
|---|---|---|
| **E** | read from the retail XBE with `h2 disasm` | the guest address |
| **V** | verified on the owner's disc | the test (`#[ignore]`) or command that checks it |
| **Code** | read from this repository | the path |
| **Test** | shown by a test on synthetic data | the test name |
| **Run** | seen in a running game | the agent API method or command, and the note with the result |
| **S** | supposed: not verified | what would verify it |
| **Design** | our choice, not a fact about the game | nothing |

Status words are **Built**, **Partly built** and **Design** (not built).

Some pages define their own scheme in an "Evidence labels" section
(`halo1/models.md` has V1 to V5, the sim-rewrite specs have C, L and O).
A page keeps its scheme when the labels carry more than the table above
can; it does not mix two schemes. Do not write "read from this repository
at `<commit>`": cite the path, which stays true.

## Template for a design page

```markdown
# <What it is>

Status: Built | Partly built | Design. <One sentence: what "partly" leaves out.>
Code: `crates/<crate>/src/<file>.rs` (<entry point>). Tests: `crates/<crate>/tests/<file>.rs`.
Switch: `<name>` (<default>), listed in [switches.md](../runtime/switches.md). Or: none.

<Two to five sentences: what this part does, what it replaces or sits
between, and the one rule that must hold.>

## How it works

<The mechanism, in the order the code runs. Name files and symbols.>

## Interfaces

<What calls it and what it calls. For a guest boundary: each entry point
with its address as a named research point, what the guest still reads,
what must still be written.>

## Rules and limits

<Invariants, what it refuses and what happens then, known gaps.>

## Evidence

<Labelled facts that are not plain from the code, each with its source.
Numbers link to notes/.>

## Not verified

<What only a game run or a disc test can show.>

## Related

<Links: the pages this one depends on, and its notes.>
```

A reference page drops "How it works" for the layout or the contract it
records. A guide is a numbered list of steps with the commands. A design
that is not built uses the same template with `Status: Design` and says in
"How it works" what will be built, in the present tense of the design
("the collision world owns...").

## Keeping docs current

The rule every agent follows is in `AGENTS.md` ("Keep the docs current").
In short: a change that alters behaviour, an interface, a switch or a path
that a doc names updates that doc in the same branch, so that the page
reads as if it had always been so.

## Checking

`scripts/docs-check.py` reads the tree and needs only git and python3. It
runs nothing from the project.

```sh
scripts/docs-check.py                  # everything
scripts/docs-check.py docs/perf        # only these paths
scripts/docs-check.py --strict docs/x  # soft findings fail too
```

| Check | Finds | Fails the run |
|---|---|---|
| `links` | relative links and `#anchors` that do not resolve | yes |
| `index` | a page that `docs/README.md` does not link | yes |
| `code` | a `docs/...md` path in a code comment that does not exist | yes |
| `code-sections` | a section a code comment cites that is no longer a heading | with `--strict` |
| `names` | an `H2_*` variable or `impl.*` switch that no code reads | with `--strict` |
| `paths` | a backticked `crates/...` path that does not exist | with `--strict` |
| `style` | a heading that records history (a date, "as built", "superseded") | with `--strict` |

`names`, `paths` and `style` skip `notes/` and `docs/reviews/`, which
describe the tree as it was. A design for unbuilt work names crates and
switches that do not exist yet, so those findings are cleared or explained,
not forced to zero.
