"""AMM Sidechain.amxd is what scripts/build-sidechain-device.py builds, byte
for byte, and it is shaped the way Live loaded it on 12.4.6: an audio effect
('aaaa'), `plugin~ 1 2 3 4` (the second audio input is the key), the gen~
code, and the parameters the server sets by name."""
import importlib.util
import json
import os
import struct
import unittest

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
BUILDER = os.path.join(ROOT, "scripts", "build-sidechain-device.py")
DEVICE = os.path.join(ROOT, "AbletonMusicMaker_Devices", "AMM Sidechain.amxd")


def builder():
    spec = importlib.util.spec_from_file_location("build_sidechain_device", BUILDER)
    module = importlib.util.module_from_spec(spec)
    os.environ.pop("AMM_DEBUG", None)
    spec.loader.exec_module(module)
    return module


def patcher(data):
    assert data[:4] == b"ampf" and data[8:12] == b"aaaa", data[:16]
    i = data.index(b"ptch")
    size = struct.unpack("<I", data[i + 4:i + 8])[0]
    return json.loads(data[i + 8:i + 8 + size].rstrip(b"\x00").decode("utf-8"))["patcher"]


class TheDevice(unittest.TestCase):
    def test_the_checked_in_file_is_what_the_builder_builds(self):
        with open(DEVICE, "rb") as handle:
            on_disk = handle.read()
        self.assertEqual(on_disk, builder().build_bytes(),
                         "AMM Sidechain.amxd is stale: run scripts/build-sidechain-device.py")

    def test_it_takes_a_key_on_channels_three_and_four(self):
        with open(DEVICE, "rb") as handle:
            p = patcher(handle.read())
        texts = [b["box"].get("text") for b in p["boxes"]]
        self.assertIn("plugin~ 1 2 3 4", texts)
        self.assertIn("plugout~ 1 2", texts)
        self.assertIn("live.remote~ @normalized 1 @smoothing 1", texts)

    def test_its_parameters_are_the_ones_the_server_sets_by_name(self):
        with open(DEVICE, "rb") as handle:
            p = patcher(handle.read())
        names = sorted(b["box"]["saved_attribute_attributes"]["valueof"]["parameter_longname"]
                       for b in p["boxes"] if "saved_attribute_attributes" in b["box"])
        self.assertEqual(names, ["Attack", "Depth", "From", "Mix", "Mode", "Ratio", "Release", "Split",
                                 "Target Device", "Target Param", "Threshold", "To"])

    def test_no_debug_read_outs_ship(self):
        with open(DEVICE, "rb") as handle:
            self.assertNotIn(b"Dbg", handle.read())


if __name__ == "__main__":
    unittest.main()
