"""From the signed-in Xbox Live menu: game_type.py TYPE VARIANT MAP_ROW_PATH CACHE_NAME."""
import sys
from menu import change_map, choose, labels, launch, normal, select_path, setup, verify

kind, variant, path, cache = sys.argv[1:]
if kind not in ("Firefight", "Siege"):
    raise ValueError("TYPE must be Firefight or Siege")
rows = list(map(normal, labels()))
assert "firefight" not in rows and "siege" not in rows, rows
choose("Custom Games")
setup()
choose("Change Rules")
choose(kind)
choose(variant)
change_map()
select_path(path)
launch(cache)
if kind == "Firefight":
    verify("firefight.state", lambda state: state.get("running"))
else:
    verify("rules.state", lambda state: state.get("running") and state.get("script") == variant)
