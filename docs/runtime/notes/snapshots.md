# Machine snapshots: measurements

Evidence for [../snapshots.md](../snapshots.md). Newest first.

## 2026-11-17: first quiescence probe, Old Mombasa

Commit: not recorded; the run was made on the build the probe was written
against, and its result was written up in `d717c78dd` ("snapshots: audit the
features of the first probe run"). The log was kept outside the repository.

Method: `H2_SNAPSHOT_PROBE=1`, headless, Old Mombasa, 5256 ticks checked.
Reproduce with the same switch on any current build and read the
`snapshot-probe: summary` and `top machine` lines (see "Running the probe"
in the design page).

Result:

- `machine_quiescent` was 0. There was one machine blocker, on every tick:
  thread 3's `boot::trace::install_bounded` continuation calling the trace
  shadow of ordinal 234 (`NtWaitForSingleObjectEx`). That is a worker thread
  waiting in the kernel for its whole life, under the kernel-call trace's
  wrapper, which is always installed.
- No after-hook or async-entry continuation was live at any tick boundary,
  no interrupt was in service and none was queued.
- The wrapper's continuation was then made rebuildable
  (`trace: kernel call return`), so a later run should show the machine
  quiescent at tick boundaries. That run has not been made.
- Host blockers were the eleven features on in that run, all unaudited at
  the time, and the host subsystems. Every feature has a policy since.
