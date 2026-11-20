import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from host_capacity import CONFIG, apply_capacity, render


class HostCapacityTests(unittest.TestCase):
    def sizes(self, cap, **settings):
        section = {
            "vars": {"LIVE_MAX_HOSTS": str(cap), **settings},
            "containers": [{"class_name": name} for name in ("HostRoom", "LargeHostRoom")],
        }
        apply_capacity(section)
        return [app["max_instances"] for app in section.get("containers", [])]

    def test_each_step_for_a_single_region(self):
        for cap, sizes in [(0, []), (4, [2, 2]), (16, [5, 3]), (64, [16, 9]),
                           (256, [58, 30]), (1024, [227, 114])]:
            self.assertEqual(self.sizes(cap), sizes)

    def test_uses_configured_regions_and_slots_of_each_class(self):
        regions = '{"weur":null,"enam":null}'
        self.assertEqual(self.sizes(16, LIVE_HOST_REGIONS=regions,
                                    LIVE_HOST_PACK_MATCHES="4"), [7, 7])
        self.assertEqual(self.sizes(64, LIVE_HOST_PACK_MATCHES="8"), [16, 10])
        self.assertEqual(self.sizes(16, LIVE_HOST_PACK_MATCHES="1"), [19, 19])
        for invalid in [5, 65, 1025, -1, "bad"]:
            with self.assertRaises(ValueError):
                self.sizes(invalid)

    def test_all_environments_and_no_hosts(self):
        for disabled in (False, True):
            config = render(CONFIG, disabled)
            for name, section in [("local", config), *config["env"].items()]:
                with self.subTest(name=name, disabled=disabled):
                    if disabled:
                        self.assertEqual(section["vars"]["LIVE_MAX_HOSTS"], "0")
                        self.assertNotIn("containers", section)
                    else:
                        self.assertEqual(section["vars"]["LIVE_MAX_HOSTS"], "4")
                        self.assertEqual({a["class_name"]: a["max_instances"]
                                          for a in section["containers"]},
                                         {"HostRoom": 2, "LargeHostRoom": 2})
                    self.assertIn("r2_buckets", section)
                    self.assertIn("durable_objects", section)
