"""Regenerate the browser's inline Clanker kernel with LLVM clang and wasm-ld."""
import base64
import hashlib
import json
import os
from pathlib import Path
import subprocess
from tempfile import TemporaryDirectory

root = Path(__file__).resolve().parents[1]
source = root / "crates/live-proto/src/clanker_appearance.c"
output = root / "web/src/lib/clanker-kernel.json"
with TemporaryDirectory(dir=os.environ.get("TMPDIR")) as scratch:
    wasm = Path(scratch) / "appearance.wasm"
    subprocess.run([
        os.environ.get("CLANG", "clang"), "--target=wasm32", "-Oz", "-ffreestanding",
        "-nostdlib", "-Wl,--no-entry", "-Wl,--strip-all", "-Wl,--export=clanker_appearance",
        "-Wl,--export=__heap_base", "-Wl,--export-memory", str(source), "-o", str(wasm),
    ], check=True)
    output.write_text(json.dumps({
        "sourceSha256": hashlib.sha256(source.read_bytes()).hexdigest(),
        "wasm": base64.b64encode(wasm.read_bytes()).decode(),
    }, indent=2) + "\n")
