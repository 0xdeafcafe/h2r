"""From the signed-in Xbox Live menu: forge.py 'Map Pack/Sanctuary' deltatap."""
import sys
from menu import change_map, choose, launch, pad, select_path, verify

path, map_name = sys.argv[1:]
choose("Forge")
verify("ui.lobby.tabs", lambda state: state.get("selected") == "forge")
change_map()
select_path(path)
launch(map_name)
verify("forge.status", lambda state: state.get("monitor") and state.get("tool") == "objects")
pad("x")
verify("forge.status", lambda state: (state.get("screen") or {}).get("name") in
       ("forge.objects", "forge.properties"))
