#!/usr/bin/env python3
"""Run against h2 play in a signed-in, English local MP lobby with Start Game available."""
import json
import re
import subprocess
import sys
import time


def agent(method, **params):
    command = ["h2", "agent", "--timeout", "185", method, json.dumps(params)]
    return json.loads(subprocess.check_output(command, text=True))


def wait(description, check, seconds=120):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = check()
        if value:
            return value
        time.sleep(0.25)
    raise RuntimeError("Timed out waiting for " + description)


def choose(pattern):
    """Read the live labels and focus. Re-audit after each delivered press."""
    def navigate():
        audit = agent("ui.audit")
        if not audit["target"] or audit["focused_row"] is None:
            return False
        for pane in audit["active"]["panes"]:
            if not pane["active"]:
                continue
            for listing in pane["lists"]:
                rows = listing["rows"]
                for row in rows:
                    if re.search(pattern, row.get("text") or "", re.I):
                        index = row["index"]
                        focus = audit["focused_row"]
                        if focus == index:
                            agent("ui.press", target=audit["target"], action="choose", row=index)
                            return True
                        agent("ui.press", target=audit["target"], action="down" if focus < index else "up")
                        return False
                count = listing.get("item_count") or 0
                if rows and rows[-1]["index"] is not None and rows[-1]["index"] < count - 1:
                    agent("ui.press", target=audit["target"], action="down")
                    return False
        return False
    wait("menu row " + pattern, navigate, 30)


def run():
    before = {row["id"] for row in agent("theater.list")}
    if not agent("theater.status")["default_recording"]:
        raise RuntimeError("The selected session needs default film recording")
    choose(r"^Start Game$")
    agent("session.wait", until={"phase": "gameplay", "map_ticks": 2100}, timeout_ms=180000)
    agent("input.pad", steps=[{"buttons": ["start"], "ms": 100, "reads": 2}])
    choose(r"^End Game$")
    # Retail asks the leader to confirm on a separate surface.
    choose(r"^(Yes|End Game)$")

    def new_film():
        status = agent("theater.status")
        if status["failure"]:
            raise RuntimeError(status["failure"])
        return next((row for row in agent("theater.list") if row["id"] not in before), None)

    film = wait("the local game's film", new_film)
    print(json.dumps(film), flush=True)
    # Close the carnage report when it is in front of the post-game lobby.
    def postgame_surface():
        audit = agent("ui.audit")
        return audit if audit.get("active") and audit.get("target") else None

    audit = wait("post-game menu", postgame_surface)
    labels = [r.get("text") for p in (audit["active"] or {}).get("panes", [])
              for l in p["lists"] for r in l["rows"]]
    if not any((s or "").lower() in ("game setup", "theater") for s in labels):
        agent("input.pad", steps=[{"buttons": ["b"], "ms": 100, "reads": 2}])
    choose(r"^Game Setup$")
    choose(r"^Theater$")
    # The newest matching map is the first result in Recent.
    name = film["map"].replace("\\", "/").split("/")[-1].removesuffix(".map")
    choose(re.escape(sys.argv[1] if len(sys.argv) > 1 else name))
    choose(r"^Watch$")

    def player_state():
        try:
            return agent("theater.state")
        except subprocess.CalledProcessError:
            return {}  # the child socket is opening or closing

    def playing():
        state = player_state()
        return state if state.get("id") == film["id"] and not state["seeking"] else False

    wait("Watch to start the selected film", playing)
    held = agent("theater.pause")["tick"]
    for mode in ("free", "follow"):
        params = {"mode": mode}
        if mode == "follow":
            params["seat"] = 0
        agent("theater.camera", **params)
        wait(mode + " camera", lambda: player_state().get("camera") == mode)
    agent("theater.step", ticks=1)
    wait("one paused step", lambda: agent("theater.state")["tick"] == held + 1)
    agent("theater.seek", tick=held)
    wait("scrub back to the held tick", lambda: (s := playing()) and s["tick"] == held and s["paused"])
    agent("theater.exit")
    wait("return to the library", lambda: player_state().get("active") is False)
    choose(r"^Save$")
    wait("Saved pin in the original game", lambda: any(
        row["id"] == film["id"] and row["saved"] for row in agent("theater.list")
    ))
    print("Returned to the film detail page and saved", film["id"])


if __name__ == "__main__":
    run()
