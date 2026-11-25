"""Tests of voice_clone.py with a fake mlx.core (no MLX, no model, no audio):
python3 -I crates/apps/studio/sidecar/test_voice_clone.py"""
import importlib.util
import os
import sys
import types
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))


class FloatDim:
    """An element of a float32 mx.array: not a shape."""

    def __int__(self):
        return 7


def fake_mlx():
    """An mlx.core whose random.normal refuses a shape of anything but ints,
    as MLX does."""
    mx = types.ModuleType("mlx.core")
    random = types.ModuleType("mlx.core.random")

    def normal(shape=(), dtype=None, loc=None, scale=None, key=None):
        if not all(type(d) is int for d in shape):
            raise TypeError("normal(): incompatible function arguments. Invoked with types: tuple")
        return ("noise", tuple(shape))

    random.normal = normal
    mx.random = random
    pkg = types.ModuleType("mlx")
    pkg.core = mx
    return {"mlx": pkg, "mlx.core": mx, "mlx.core.random": random}


def load():
    spec = importlib.util.spec_from_file_location("voice_clone", os.path.join(HERE, "voice_clone.py"))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


class WholeNumberShapes(unittest.TestCase):
    def setUp(self):
        self.saved = {k: sys.modules.get(k) for k in ("mlx", "mlx.core", "mlx.core.random")}
        sys.modules.update(fake_mlx())
        self.mod = load()

    def tearDown(self):
        for k, v in self.saved.items():
            if v is None:
                sys.modules.pop(k, None)
            else:
                sys.modules[k] = v

    def test_the_unpatched_call_fails_as_reported(self):
        import mlx.core as mx

        with self.assertRaises(TypeError):
            mx.random.normal((256, FloatDim()))

    def test_a_float_duration_is_made_a_whole_number(self):
        import mlx.core as mx

        self.mod.whole_number_shapes()
        self.assertEqual(mx.random.normal((256, FloatDim())), ("noise", (256, 7)))
        self.assertEqual(mx.random.normal(shape=(2, 3)), ("noise", (2, 3)))

    def test_patching_twice_wraps_once(self):
        import mlx.core as mx

        self.mod.whole_number_shapes()
        first = mx.random.normal
        self.mod.whole_number_shapes()
        self.assertIs(mx.random.normal, first)

    def test_a_shape_that_is_no_sequence_is_left_to_mlx(self):
        import mlx.core as mx

        self.mod.whole_number_shapes()
        with self.assertRaises(TypeError):
            mx.random.normal(None)


if __name__ == "__main__":
    unittest.main()
