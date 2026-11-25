"""From a pregame lobby: maps.py [expected.json]. Without a baseline, prints one."""
import json
import sys
from menu import change_map, choose, labels, normal, pad, surface

change_map()
rows = []
if surface().get("mode") == "folders":
    for folder in labels():
        choose(folder)
        members = labels()
        rows.append([folder, members])
        if normal(folder).startswith("mods"):
            for member in members:
                if normal(member) == "useoriginalmaps":
                    continue
                choose(member)
                if surface().get("mode") == "pack":
                    rows.append([[folder, member], labels()])
                    pad("b")
                else:
                    # A loose map applies to the lobby. Reopen the same folder.
                    change_map()
                    choose(folder)
        pad("b")
else:
    rows.append([surface().get("folder"), labels()])
print(json.dumps(rows, ensure_ascii=False, indent=2))
if len(sys.argv) > 1:
    with open(sys.argv[1], encoding="utf8") as baseline:
        if rows != json.load(baseline):
            sys.exit("Map list differs from the baseline (contents or order).")
