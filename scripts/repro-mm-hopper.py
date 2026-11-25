#!/usr/bin/env python3
"""Start signed in at the Xbox Live menu on a local Live server's default lineup."""
import argparse
import json
import subprocess
import sys
import time


def call(method, **params):
    command = ["h2", "agent"]
    if args.socket:
        command.extend(["--socket", args.socket])
    out = subprocess.check_output([*command, "--timeout", "15", method, json.dumps(params)])
    return json.loads(out)


def rows(audit):
    return {row["index"]: [l["text"] for l in row.get("labels", []) if l.get("enabled") and l.get("text")]
            or [row.get("text") or ""] for pane in (audit["active"] or {}).get("panes", [])
            if pane.get("active") for listing in pane["lists"] for row in listing["rows"]}


def ticks(n):
    call("session.wait", until={"ticks": n}, timeout_ms=10000)


def choose(label):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        audit = call("ui.audit")
        visible = rows(audit)
        focus = audit["focused_row"]
        wanted = next((i for i, text in visible.items()
                       if any(label.casefold() == part.strip().casefold() for part in text)), None)
        if wanted is None or focus is None or not audit["target"]:
            ticks(4)
            continue
        action = "choose" if wanted == focus else ("down" if wanted > focus else "up")
        ack = call("ui.press", target=audit["target"], row=focus, action=action)
        if not ack["delivered"]:
            raise RuntimeError(f"Undelivered {action} on {label}")
        ticks(4)
        if action == "choose":
            return
    raise RuntimeError(f"Could not choose {label}: {audit}")


def run():
    prior = call("live.matchmaking.trace")["transitions"]
    baseline = max((r["sequence"] for r in prior), default=0)
    choose("Clankermatch")
    choose("Clanker Doubles")
    deadline = time.monotonic() + 240
    saw_search = saw_phantom = started = False
    while time.monotonic() < deadline:
        trace = call("live.matchmaking.trace")["transitions"]
        for row in trace:
            if row["sequence"] <= baseline:
                continue
            state = row["state"]
            for key in ["squad", "target"]:
                session = state.get(key) or {}
                if session.get("hopper") == 10:
                    saw_search |= state["lifecycle"] == 6
                    saw_phantom |= any(p.get("phantom") for p in session["peers"])
        status = call("session.status")["at"]
        if saw_search and saw_phantom and status["phase"] == "gameplay":
            print(json.dumps({"playlist": "Clanker Doubles", "hopper": 10, "at": status}))
            return
        if not saw_search and not started:
            labels = [text.strip() for texts in rows(call("ui.audit")).values() for text in texts]
            start = next((t for t in labels if t.casefold() in {"start game", "start matchmaking", "find match"}), None)
            if start:
                choose(start)
                started = True
        ticks(15)
    raise RuntimeError(f"Did not reach gameplay with phantoms: search={saw_search}, phantom={saw_phantom}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--socket")
    args = parser.parse_args()
    try:
        run()
    except (RuntimeError, subprocess.SubprocessError) as error:
        print(error, file=sys.stderr)
        for method in ("ui.audit", "live.matchmaking.trace"):
            print(json.dumps(call(method)), file=sys.stderr)
        sys.exit(1)
