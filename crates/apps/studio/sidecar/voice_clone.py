"""Studio's local voice sidecar: clones a game character's voice on this Mac.

Studio (crates/apps/studio/src/voice/sidecar.rs) writes this file into a
venv under its caches dir, installs requirements-<backend>.txt there, and
runs it with `python -I -u voice_clone.py --backend <id>`. It reads one JSON
request per line on stdin, {"id", "op", "params"}, and answers on stdout
with progress lines, {"id", "progress": {stage, done, total, text}}, and one
result line, {"id", "ok": true, "result": {...}} or {"id", "ok": false,
"error": "..."}. Anything else it or its libraries print goes to stderr,
which Studio keeps in sidecar.log.

Ops:
  hello       the backend and the library versions
  prepare     downloads the model's and the Whisper's weights (the only op
              Studio runs with the network allowed; every other op runs
              with HF_HUB_OFFLINE=1)
  say         {text, speed, reference: {path, text}, checkpoint?, out}:
              writes the line as a WAV at `out`
  transcribe  {files, language}: each 24 kHz clip's text, by the local
              Whisper
  train       {manifest, out, steps?}: fine-tunes on the manifest's lines
              and writes the weights under `out`

No audio leaves this machine: nothing here uploads, and the weights are
read from HF_HOME. docs/editor/voice-cloning.md.
"""

import argparse
import json
import sys
import traceback
import wave

# Library output must not reach the protocol: keep stdout for our lines.
OUT = sys.stdout
sys.stdout = sys.stderr

WHISPER = "mlx-community/whisper-large-v3-turbo"


def send(obj):
    OUT.write(json.dumps(obj) + "\n")
    OUT.flush()


class Progress:
    def __init__(self, rid):
        self.rid = rid

    def __call__(self, stage, done=0, total=0, text=None):
        send({"id": self.rid, "progress": {"stage": stage, "done": done, "total": total, "text": text}})


class Failure(Exception):
    """A failure whose message is the whole story."""


def read_wav(path):
    """A 16-bit PCM WAV as mono float32 samples, and its rate."""
    import numpy as np

    with wave.open(path, "rb") as w:
        if w.getsampwidth() != 2:
            raise Failure(f"{path}: not 16-bit PCM")
        ch = w.getnchannels()
        rate = w.getframerate()
        data = np.frombuffer(w.readframes(w.getnframes()), dtype="<i2").astype(np.float32) / 32768.0
    if ch > 1:
        data = data.reshape(-1, ch).mean(axis=1)
    return data, rate


def write_wav(path, samples, rate):
    """Mono float samples as a 16-bit PCM WAV."""
    import numpy as np

    s = np.clip(np.asarray(samples, dtype=np.float32).reshape(-1), -1.0, 1.0)
    pcm = (s * 32767.0).round().astype("<i2")
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(int(rate))
        w.writeframes(pcm.tobytes())


def resample(x, rate, to):
    import numpy as np

    if rate == to or len(x) == 0:
        return x
    n = int(round(len(x) * to / rate))
    return np.interp(np.arange(n) * (rate / to), np.arange(len(x)), x).astype(np.float32)


def download(repo, p):
    from huggingface_hub import snapshot_download

    p("weights", 0, 1, f"downloading {repo}")
    snapshot_download(repo_id=repo)
    p("weights", 1, 1, f"have {repo}")


def transcribe(params, p):
    import mlx_whisper

    files = params.get("files") or []
    language = params.get("language") or "en"
    texts = []
    for i, f in enumerate(files):
        audio, rate = read_wav(f)
        # The Whisper takes 16 kHz samples; an array needs no ffmpeg.
        r = mlx_whisper.transcribe(resample(audio, rate, 16000), path_or_hf_repo=WHISPER, language=language)
        texts.append({"path": f, "text": (r.get("text") or "").strip()})
        p("transcribe", i + 1, len(files))
    return {"texts": texts}


def manifest_lines(path):
    with open(path, encoding="utf-8") as f:
        lines = [json.loads(l) for l in f if l.strip()]
    lines = [l for l in lines if l.get("text") and l.get("text_from") in ("subtitle", "asr")]
    if not lines:
        raise Failure(f"{path}: no line with text to train on")
    return lines


def whole_number_shapes():
    """f5-tts-mlx's sampler draws its noise as mx.random.normal((channels,
    dur)) with `dur` an element of a float32 mx.array of durations, which
    current MLX refuses as a shape ("normal(): incompatible function
    arguments ... Invoked with types: tuple"). Every shape handed to
    mx.random.normal is made of Python ints from here on. Safe to call more
    than once."""
    import mlx.core as mx

    random = mx.random
    if getattr(random.normal, "whole_number_shapes", False):
        return
    original = random.normal

    def normal(shape=(), *args, **kwargs):
        try:
            shape = tuple(int(d) for d in shape)
        except TypeError:
            pass  # not a sequence: let MLX say what is wrong
        return original(shape, *args, **kwargs)

    normal.whole_number_shapes = True
    random.normal = normal


class F5Mlx:
    """F5-TTS through its MLX port, f5-tts-mlx (code MIT; weights CC-BY-NC-4.0)."""

    BASE = "lucasnewman/f5-tts-mlx"
    VOCODER = "lucasnewman/vocos-mel-24khz"

    def __init__(self):
        self.models = {}

    def hello(self):
        import importlib.metadata as md

        return {"backend": "f5-mlx", "f5-tts-mlx": md.version("f5-tts-mlx"), "mlx": md.version("mlx")}

    def prepare(self, p):
        for repo in (self.BASE, self.VOCODER, WHISPER):
            download(repo, p)
        return {"weights": [self.BASE, self.VOCODER, WHISPER]}

    def model_class(self):
        # The class generate() loads through (f5_tts_mlx/generate.py).
        from f5_tts_mlx.cfm import F5TTS

        return F5TTS

    def load(self, checkpoint):
        """The base model, with a fine-tune's weights over it; kept loaded."""
        key = checkpoint or ""
        if key not in self.models:
            F5TTS = self.model_class()
            m = F5TTS.from_pretrained(self.BASE)
            if checkpoint:
                m.load_weights(checkpoint, strict=False)
            self.models[key] = m
        return self.models[key]

    def say(self, params, p):
        import f5_tts_mlx.generate as g

        whole_number_shapes()
        ref = params["reference"]
        checkpoint = params.get("checkpoint")
        model = self.load(checkpoint)
        F5TTS = self.model_class()
        # generate() loads the model by name each call: hand it ours.
        original = F5TTS.from_pretrained
        F5TTS.from_pretrained = staticmethod(lambda *a, **k: model)
        try:
            g.generate(
                generation_text=params["text"],
                ref_audio_path=ref["path"],
                ref_audio_text=ref["text"],
                speed=float(params.get("speed") or 1.0),
                output_path=params["out"],
            )
        finally:
            F5TTS.from_pretrained = original
        return {"out": params["out"], "rate": 24000}

    def train(self, params, p):
        """A fine-tune of the pretrained model, as f5_tts_mlx.trainer runs its
        own (loss = model(mel_spec, text=, lens=), AdamW, gradients clipped
        at 1), one clip per step, the lines shuffled. The trainable
        parameters are saved as f5_tts_mlx.trainer saves a checkpoint."""
        import os
        import random

        import mlx.core as mx
        import mlx.nn as nn
        import mlx.optimizers as optim
        from mlx.utils import tree_flatten

        whole_number_shapes()
        try:
            from f5_tts_mlx.audio import log_mel_spectrogram
        except ImportError as e:
            raise Failure(f"this f5-tts-mlx has no log_mel_spectrogram ({e}): docs/editor/voice-cloning.md, 'The fine-tune'")

        lines = manifest_lines(params["manifest"])
        steps = int(params.get("steps") or max(2000, 4 * len(lines)))
        out = params["out"]
        model = self.model_class().from_pretrained(self.BASE)
        model.train()
        opt = optim.AdamW(learning_rate=float(params.get("learning_rate") or 1e-5))

        def loss_fn(m, mel, text, lens):
            r = m(mel, text=text, lens=lens)
            return r[0] if isinstance(r, tuple) else r

        step = nn.value_and_grad(model, loss_fn)
        order = list(range(len(lines)))
        rng = random.Random(0)
        losses = []
        for i in range(steps):
            if i % len(order) == 0:
                rng.shuffle(order)
            line = lines[order[i % len(order)]]
            audio, rate = read_wav(line["path"])
            mel = log_mel_spectrogram(mx.array(resample(audio, rate, 24000)))
            if mel.ndim == 2:
                mel = mel[None]
            lens = mx.array([mel.shape[1]], dtype=mx.int32)
            try:
                loss, grads = step(model, mel, [line["text"]], lens)
            except (TypeError, ValueError) as e:
                raise Failure(
                    f"this f5-tts-mlx's model did not take (mel_spec, text, lens) for training ({e}); "
                    "docs/editor/voice-cloning.md, 'The fine-tune'"
                )
            grads, _ = optim.clip_grad_norm(grads, 1.0)
            opt.update(model, grads)
            mx.eval(model.parameters(), opt.state)
            losses.append(float(loss))
            if (i + 1) % 10 == 0 or i + 1 == steps:
                recent = sum(losses[-50:]) / len(losses[-50:])
                p("train", i + 1, steps, f"loss {recent:.4f}")
        os.makedirs(out, exist_ok=True)
        weights = os.path.join(out, "model.safetensors")
        mx.save_safetensors(weights, dict(tree_flatten(model.trainable_parameters())))
        return {"checkpoint": weights, "steps": steps, "loss": sum(losses[-50:]) / max(1, len(losses[-50:]))}


class Qwen3Tts:
    """Qwen3-TTS's base model, qwen-tts on PyTorch MPS (Apache-2.0): zero-shot only."""

    BASE = "Qwen/Qwen3-TTS-12Hz-1.7B-Base"

    def __init__(self):
        self.model = None

    def hello(self):
        import importlib.metadata as md

        return {"backend": "qwen3-tts", "qwen-tts": md.version("qwen-tts"), "torch": md.version("torch")}

    def prepare(self, p):
        for repo in (self.BASE, WHISPER):
            download(repo, p)
        return {"weights": [self.BASE, WHISPER]}

    def say(self, params, p):
        import torch
        from qwen_tts import Qwen3TTSModel

        if params.get("checkpoint"):
            raise Failure("qwen3-tts has no fine-tune here")
        if self.model is None:
            device = "mps" if torch.backends.mps.is_available() else "cpu"
            self.model = Qwen3TTSModel.from_pretrained(self.BASE, device_map=device, dtype=torch.float16)
        ref = params["reference"]
        wavs, rate = self.model.generate_voice_clone(
            text=params["text"],
            language="English",
            ref_audio=ref["path"],
            ref_text=ref["text"],
        )
        write_wav(params["out"], wavs[0], rate)
        return {"out": params["out"], "rate": rate}

    def train(self, params, p):
        raise Failure("qwen3-tts has no fine-tune here: it clones from a reference only")


BACKENDS = {"f5-mlx": F5Mlx, "qwen3-tts": Qwen3Tts}


def run(backend, op, params, p):
    if op == "hello":
        return backend.hello()
    if op == "prepare":
        return backend.prepare(p)
    if op == "say":
        return backend.say(params, p)
    if op == "transcribe":
        return transcribe(params, p)
    if op == "train":
        return backend.train(params, p)
    raise Failure(f"no op {op!r}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--backend", required=True, choices=sorted(BACKENDS))
    args = ap.parse_args()
    backend = BACKENDS[args.backend]()
    for raw in sys.stdin:
        raw = raw.strip()
        if not raw:
            continue
        try:
            req = json.loads(raw)
        except ValueError:
            continue
        rid = req.get("id")
        try:
            result = run(backend, req.get("op"), req.get("params") or {}, Progress(rid))
            send({"id": rid, "ok": True, "result": result})
        except Failure as e:
            send({"id": rid, "ok": False, "error": str(e)})
        except Exception as e:  # noqa: BLE001 - every failure is answered
            traceback.print_exc()
            send({"id": rid, "ok": False, "error": f"{type(e).__name__}: {e}"})


if __name__ == "__main__":
    main()
