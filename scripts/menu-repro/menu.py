"""Drive the English menus on the game selected by H2_AGENT."""
import json
import subprocess
import sys
import time
import unicodedata


def rpc(method, **params):
    raw = subprocess.check_output([
        "h2", "agent", "--timeout", "95", method, json.dumps(params),
    ], text=True)
    return json.loads(raw)


def wait(ticks=12):
    rpc("session.wait", until={"ticks": ticks}, timeout_ms=15000)


def pad(button):
    rpc("input.pad", player=0, steps=[
        {"buttons": [button], "ticks": 2}, {"buttons": [], "ticks": 2},
    ])
    wait()


def surface():
    stack = rpc("ui.stack")
    owner = stack.get("input") or {}
    for channel in stack["channels"]:
        active = channel.get("active") or {}
        if active.get("instance") == owner.get("instance") and owner:
            return active
    raise RuntimeError(f"No active input surface: {stack}")


def view():
    for _ in range(16):
        audit = rpc("ui.audit")
        for pane in (audit.get("active") or {}).get("panes", []):
            for group in pane["lists"]:
                focus = group.get("focused_row")
                if focus is not None and group["rows"]:
                    return focus, group["item_count"], {
                        row["index"]: row["text"] for row in group["rows"] if row.get("text")
                    }
        wait(4)
    raise RuntimeError(f"No focused list on {surface()}")


def focus_row(index):
    for _ in range(100):
        focus, count, rows = view()
        if not 0 <= index < count:
            raise RuntimeError(f"Row {index} outside {count} on {surface()}")
        if focus == index:
            return rows[focus]
        pad("up" if focus > index else "down")
    raise RuntimeError(f"Could not focus row {index}")


def labels():
    focus, count, rows = view()
    for index in range(count):
        if index not in rows:
            rows[index] = focus_row(index)
    focus_row(focus)
    return [rows[i] for i in range(count)]


def normal(text):
    return "".join(c for c in text.casefold() if unicodedata.category(c)[0] in "LN")


def choose(label):
    rows = labels()
    matches = [i for i, text in enumerate(rows) if normal(text) == normal(label)]
    if len(matches) != 1:
        raise RuntimeError(f"Expected one {label!r} in {rows} on {surface()}")
    focus_row(matches[0])
    print(f"A on {label} (screen {surface()['id']})", file=sys.stderr)
    pad("a")


def setup():
    if surface()["id"] != 23:
        choose("Game Setup")


def change_map():
    setup()
    choose("Change Map")


def select_path(path):
    for row in path.split("/"):
        choose(row)


def launch(map_name):
    if surface()["id"] == 23:
        pad("b")
    choose("Start Game")
    rpc("session.wait", until={"phase": "gameplay", "map": map_name, "map_ticks": 30},
        timeout_ms=90000)
    print(json.dumps(rpc("session.status")), file=sys.stderr)


def verify(method, predicate):
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        state = rpc(method)
        if predicate(state):
            print(json.dumps(state, indent=2))
            return state
        wait()
    raise RuntimeError(f"{method} did not reach the expected state: {state}")
