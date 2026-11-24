"""Round-trip named menu rows on an already running game via h2 agent.

Start on the common parent of the named rows. Each argument is one row
label. Exercise siblings in both orders, including scrolling off-screen.
"""

import json
import subprocess
import sys
import time


def agent(method, *args):
    result = subprocess.check_output(["h2", "agent", method, *args], text=True)
    return json.loads(result)


def surface(audit):
    active = audit["active"]
    return tuple(active.get(field) for field in ("window", "slot", "id", "name"))


def settled(previous=None):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        audit = agent("ui.audit")
        stack = agent("ui.stack")
        if (audit.get("target") and (stack["input"] or {}).get("instance") == audit["active"]["address"]
                and (previous is None or surface(audit) != previous)
                and all(not ch["pending"] and not ch["outgoing"] for ch in stack["channels"])):
            return audit, stack
        time.sleep(0.1)
    raise RuntimeError(f"menu did not settle: {stack}")


def press(audit, action):
    args = {"target": audit["target"], "action": action}
    if action == "choose":
        args["row"] = audit["focused_row"]
    if not agent("ui.press", json.dumps(args))["delivered"]:
        raise RuntimeError(f"{action} was not delivered")


def focus(label):
    audit, _ = settled()
    seen = set()
    direction = "down"
    while True:
        index = audit["focused_row"]
        for pane in audit["active"]["panes"]:
            if not pane["active"]:
                continue
            for listing in pane["lists"]:
                if listing["focused_row"] != index:
                    continue
                for row in listing["rows"]:
                    if row["index"] == index and (row["text"] or "").strip().casefold() == label.casefold():
                        return audit
        if index in seen:
            if direction == "up":
                raise RuntimeError(f"row {label!r} unavailable")
            direction = "up"
            seen.clear()
        seen.add(index)
        press(audit, direction)
        audit, _ = settled()


def stack_shape(stack):
    def identity(node):
        return None if not node else (node["id"], node["mode"], node["folder"], node["parent"])
    return [(ch["window"], ch["slot"], identity(ch["active"]), ch["history"]) for ch in stack["channels"]]


def round_trip(label):
    parent = focus(label)
    before = agent("ui.stack")
    press(parent, "choose")
    child, opened = settled(surface(parent))
    target = (opened["back"] or {}).get("parent")
    if target and target["id"] != parent["active"]["id"]:
        raise RuntimeError(f"{label}: wrong Back target {target}")
    press(child, "back")
    restored, after = settled(surface(child))
    if (surface(parent) != surface(restored) or stack_shape(before) != stack_shape(after)
            or parent["focused_row"] != restored["focused_row"]):
        raise RuntimeError(f"{label}: Back restored {after}, expected {before}")
    print(f"{label}: returned to {surface(restored)}, row {restored['focused_row']}")


if __name__ == "__main__":
    labels = sys.argv[1:]
    if not labels:
        sys.exit("usage: ui-mode-back.py ROW [ROW ...]")
    for label in labels + labels[::-1]:
        round_trip(label)
