"""Render the Worker config with each container application's lease-derived limit."""

import argparse
import json
from pathlib import Path
import sys
import tempfile
import tomllib

CONFIG = Path(__file__).with_name("wrangler.toml")
CLASS_SLOTS = {"HostRoom": 5, "LargeHostRoom": 10}


def apply_capacity(section, disabled=False):
    settings = section["vars"]
    cap = 0 if disabled else int(settings.get("LIVE_MAX_HOSTS", "4"))
    if not (0 <= cap <= 4 or cap in (16, 64, 256, 1024)):
        raise ValueError("LIVE_MAX_HOSTS must be 0–4, 16, 64, 256 or 1024")
    settings["LIVE_MAX_HOSTS"] = str(cap)
    if cap == 0:
        section.pop("containers", None)
        return

    regions = json.loads(settings.get("LIVE_HOST_REGIONS", "{}"))
    if not isinstance(regions, dict):
        raise ValueError("LIVE_HOST_REGIONS must be a JSON object")
    # An unconfigured region table uses a single regional pool.
    region_count = max(len(regions), 1)
    pack_setting = settings.get("LIVE_HOST_PACK_MATCHES", "auto").strip()
    for app in section["containers"]:
        maximum = CLASS_SLOTS[app["class_name"]]
        slots = maximum if pack_setting == "auto" else min(max(1, int(pack_setting)), maximum)
        # ceil(1.1 * cap / slots), using integers even at exact multiples.
        app["max_instances"] = (11 * cap + 10 * slots - 1) // (10 * slots) + region_count


def render(path, disabled=False):
    with path.open("rb") as source:
        config = tomllib.load(source)
    for section in [config, *config.get("env", {}).values()]:
        apply_capacity(section, disabled)
    return config


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--no-hosts", action="store_true")
    parser.add_argument("--deploy-file", action="store_true",
                        help="write a temporary sibling of wrangler.toml and print its path")
    args = parser.parse_args()
    try:
        config = render(CONFIG, args.no_hosts)
    except (KeyError, ValueError, TypeError) as error:
        sys.exit(f"host capacity in {CONFIG.name}: {error}")
    if args.deploy_file:
        # Wrangler resolves image, main and asset paths relative to this file.
        with tempfile.NamedTemporaryFile(mode="w", dir=CONFIG.parent, prefix=".wrangler-hosts-",
                                         suffix=".json", delete=False) as output:
            json.dump(config, output)
        print(output.name)
    else:
        print(json.dumps(config, indent=2))
