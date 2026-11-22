#!/usr/bin/env python3
"""Compare two runs from the same local gameplay snapshot, with one input log.

Use a disposable offline game and a windowed editor. Keep the physical pads
idle. The game must support snapshots, input.replay, sim.hashes and live.*.
The replay must cover --ticks ticks starting one tick after the snapshot.
Both endpoints are agent sockets. The game endpoint may also be tcp://host:port.
Print aggregate measurements only. The game resumes after the second pass.
"""

import argparse
import contextlib
import json
import socket
import sys
import time
from urllib.parse import urlsplit


@contextlib.contextmanager
def rpc(endpoint):
    if endpoint.startswith("tcp://"):
        addr = urlsplit(endpoint)
        sock = socket.create_connection((addr.hostname, addr.port), timeout=120)
    else:
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.settimeout(120)
        sock.connect(endpoint)
    with sock, sock.makefile("rb") as replies:
        serial = 0

        def call(method, **params):
            nonlocal serial
            serial += 1
            req = {"jsonrpc": "2.0", "id": serial, "method": method, "params": params}
            sock.sendall(json.dumps(req).encode() + b"\n")
            line = replies.readline()
            if not line:
                raise RuntimeError(f"{method}: socket closed")
            reply = json.loads(line)
            if reply.get("id") != serial:
                raise RuntimeError(f"{method}: unexpected reply id")
            if "error" in reply:
                raise RuntimeError(f"{method}: {reply['error']}")
            return reply["result"]

        yield call


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def wait(read, ready, description):
    deadline = time.monotonic() + 15
    while True:
        result = read()
        if ready(result):
            return result
        if time.monotonic() >= deadline:
            raise RuntimeError(f"timed out waiting for {description}")
        time.sleep(0.03)


def load_paused(game, snapshot):
    loaded = game("state.load", name=snapshot)
    require(loaded["hash"]["checked"] and loaded["hash"]["equal"], "snapshot RAM hash mismatch")
    # state.load completes between slices. Wait until the game has actually
    # returned to the tick observer, even when both passes start at the same tick.
    at = wait(lambda: game("session.status")["at"],
              lambda a: a["paused"] and a["ticks"] > loaded["at"]["ticks"],
              "the first held tick after the snapshot")
    require(at["phase"] == "gameplay" and at["tick"] == loaded["tick"] + 1,
            "snapshot did not restore to the expected gameplay tick")
    return at["tick"]


def replay(game, name, start, ticks):
    game("input.replay", name=name)
    try:
        began = time.monotonic()
        last = game("session.step", ticks=ticks)["at"]
        elapsed = time.monotonic() - began
    finally:
        counts = game("input.stop")
    require(last["paused"] and last["tick"] == start + ticks, "step missed the requested tick")
    require(counts["matched"] == counts["replayed"] > 0 and counts["past_end"] == 0,
            "input replay ran out, read no inputs, or saw different controller handles")
    rows = game("sim.hashes", from_tick=start + 1, max=ticks)["rows"]
    require([r["tick"] for r in rows] == list(range(start + 1, start + ticks + 1)),
            "sim.hashes did not cover every requested tick")
    return rows, counts["replayed"], elapsed


def compare_objects(game, editor, tick):
    wait(lambda: editor("mirror.status"),
         lambda v: v["store"]["tick"] == tick, f"the editor to apply tick {tick}")
    mir = editor("mirror.objects", at_tick=tick, limit=2048)
    sim = game("sim.objects", max=2048)
    require(sim["at"]["tick"] == mir["tick"] == tick and sim["at"]["paused"],
            "object results have different stamps")
    require(not sim["truncated"] and not mir["truncated"], "incomplete object list")
    flag_names = ("active", "awake", "post", "new", "deleting", "defer", "attach", "child")
    common = ("kind", "parent", "forward", "up", "scale", "velocity")
    expected = {o["id"]: ([o[k] for k in common], o["definition"], o["position"],
                          o["vitality"]["body"], o["vitality"]["shield"], set(o["flags"]))
                for o in sim["objects"]}
    actual = {o["id"]: ([o[k] for k in common], o["tag"], o["pos"], o["body"], o["shield"],
                        {name for i, name in enumerate(flag_names) if o["flags"] & (1 << i)})
              for o in mir["objects"]}
    require(expected and len(actual) == len(mir["objects"]) and len(expected) == len(sim["objects"]),
            "empty pool or repeated salted object handle")
    require(expected.keys() == actual.keys(), f"object handle sets differ at tick {tick}")
    for handle in expected:
        require(expected[handle] == actual[handle], f"object {handle} differs at tick {tick}")
    return len(actual)


def check(game, editor, args):
    pair = editor("pair.status")
    original = pair.get("socket") if pair["state"] == "connected" else None
    require(pair["state"] == "off" or original == args.game,
            "use an idle editor window or one paired to the game under test")
    require(game("input.state")["mode"] == "live", "an input recording or replay is active")
    game("session.claim")
    with contextlib.ExitStack() as cleanup:
        cleanup.callback(game, "session.release")
        game("session.pause", max_ms=600000)
        editor("pair.auto", enabled=False)
        cleanup.callback(editor, "pair.auto", enabled=pair["auto_pair"])
        if original:
            cleanup.callback(editor, "pair.connect", socket=original)
            editor("pair.disconnect")
            wait(lambda: editor("pair.status"), lambda p: p["state"] == "off", "unpair")
        wait(lambda: game("live.status"), lambda s: not s["streams"], "zero subscribers")
        start = load_paused(game, args.snapshot)
        off, reads, _ = replay(game, args.replay, start, args.ticks)
        require(not game("live.status")["streams"], "subscriber appeared during the baseline")

        require(load_paused(game, args.snapshot) == start, "snapshot start tick changed")
        editor("pair.connect", socket=args.game)
        cleanup.callback(editor, "pair.disconnect")
        status = wait(lambda: game("live.status"), lambda s: len(s["streams"]) == 1,
                      "the editor subscription")
        stream_id = status["streams"][0]["stream"]
        # Subscription takes an initial key even when the game is paused.
        # Wait for it so its bytes are outside the replay measurement.
        compare_objects(game, editor, start)
        meta = editor("mirror.status")["stream"]
        tick_rate = meta["tick_rate"]
        require(meta["stream"] == stream_id and tick_rate > 0, "invalid stream tick rate")
        before = game("live.status")["streams"][0]
        require(before["stream"] == stream_id and before["frames"] >= 1, "missing initial key")
        require(set(before["layers"]) == {"objects", "players"} and before["every"] == 1,
                "expected both mirror layers at every tick")
        on, on_reads, elapsed = replay(game, args.replay, start, args.ticks)
        require(reads == on_reads, "replay read counts differ")
        for a, b in zip(off, on):
            require(a == b, f"sim.hashes differs at tick {a['tick']}")

        end = start + args.ticks

        def accounted(stream):
            return stream["frames"] + stream["dropped"] + stream["skipped"]

        # A step can finish before its encoder/writer. Each subscribed tick
        # yields a completed write, a dropped frame, or a skipped capture.
        status = wait(lambda: game("live.status"),
                      lambda v: len(v["streams"]) == 1 and
                      accounted(v["streams"][0]) - accounted(before) >= args.ticks,
                      "the writer to finish the replay frames")
        streams = status["streams"]
        require(len(streams) == 1 and streams[0]["stream"] == before["stream"],
                "subscriber changed during measurement")
        after = streams[0]
        byte_count = after["bytes"] - before["bytes"]
        require(byte_count > 0 and after["frames"] > before["frames"], "stream sent no frames")
        cost = status["capture_us"]
        # Dropping the final replay frame is legal. Once the worker has
        # drained, take an isolated step for a precise second comparison.
        sampled = game("session.step", ticks=1)["at"]["tick"]
        objects = compare_objects(game, editor, sampled)
        print(json.dumps({
            "first_tick": start + 1, "last_tick": end, "equal_hash_rows": len(on),
            "object_tick": sampled, "equal_objects": objects, "replayed_reads": reads,
            "frames": after["frames"] - before["frames"], "bytes": byte_count,
            "tick_rate": tick_rate, "simulated_seconds": args.ticks / tick_rate,
            "bytes_per_simulated_second": round(byte_count * tick_rate / args.ticks),
            "step_wall_seconds": elapsed, "bytes_per_wall_second": round(byte_count / elapsed),
            "dropped": after["dropped"] - before["dropped"],
            "skipped": after["skipped"] - before["skipped"], "capture_us": cost,
            "within_capture_budget": cost["p99"] <= cost["budget"],
        }, indent=2))
        require(cost["p99"] <= cost["budget"], "capture p99 exceeded its budget")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--game", required=True, help="game agent endpoint")
    parser.add_argument("--editor", required=True, help="windowed editor's Unix socket")
    parser.add_argument("--snapshot", required=True, help="existing state.save name")
    parser.add_argument("--replay", required=True, help="existing input.record name")
    parser.add_argument("--ticks", type=int, default=300, help="replay ticks (1..1800)")
    args = parser.parse_args()
    if not 1 <= args.ticks <= 1800:
        parser.error("--ticks must be between 1 and 1800")
    with rpc(args.game) as game, rpc(args.editor) as editor:
        check(game, editor, args)


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ValueError, KeyError) as error:
        sys.exit(str(error))
