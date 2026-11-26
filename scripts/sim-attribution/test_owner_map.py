"""Pins owner_map ranges: AI code has its own owners, one per layer."""
import os, sys, unittest
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import owner_map


class OwnerMap(unittest.TestCase):
    def test_ranges_ascend(self):
        starts = [r[0] for r in owner_map.R]
        self.assertEqual(starts, sorted(set(starts)))

    def test_ai_layers(self):
        cases = {
            0x1a7cc0: 'ai.behav', 0x1c2120: 'ai.behav', 0x26e400: 'ai.behav',
            0x1c83c0: 'ai.top', 0x1e2f50: 'ai.actors', 0x1e3b60: 'ai.actors',
            0x1f3000: 'ai.action', 0x1fc160: 'ai.action', 0x296600: 'ai.action',
            0x1fb7e0: 'ai.recanim', 0x201c80: 'ai.squads', 0x28dab0: 'ai.squads',
            0x20ba60: 'ai.dialogue', 0x210180: 'ai.path', 0x270930: 'ai.path',
            0x2c2060: 'ai.path', 0x2c41b0: 'ai.path', 0x25a380: 'ai.perception',
            0x265d30: 'ai.target', 0x268510: 'ai.clumps', 0x26c180: 'ai.firepos',
            0x257100: 'ai.cs', 0x276050: 'ai.cs', 0x29ed00: 'ai.hs',
        }
        for addr, want in cases.items():
            self.assertEqual(owner_map.owner(addr)[0], want, hex(addr))

    def test_neighbours_are_not_ai(self):
        for addr, want in {0x1a1000: 'players', 0x1c25a0: 'havok_glue',
                           0x1c6000: 'anim', 0x1e5600: 'havok_glue',
                           0x205be0: 'veh', 0x208000: 'hs', 0x24a000: 'ui',
                           0x2c4e10: 'ui', 0x2d0000: 'crt'}.items():
            self.assertEqual(owner_map.owner(addr)[0], want, hex(addr))


if __name__ == '__main__':
    unittest.main()
