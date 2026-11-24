"""From a pregame lobby: maps.py [expected.json]. Without a baseline, prints one."""
import json
import sys
from menu import change_map, choose, labels, pad, surface

change_map()
rows = []
if surface().get("mode") == "folders":
    for folder in labels():
        choose(folder)
        rows.append([folder, labels()])
        pad("b")
else:
    rows.append([surface().get("folder"), labels()])
print(json.dumps(rows, ensure_ascii=False, indent=2))
if len(sys.argv) > 1:
    with open(sys.argv[1], encoding="utf8") as baseline:
        if rows != json.load(baseline):
            sys.exit("Map list differs from the baseline (contents or order).")
