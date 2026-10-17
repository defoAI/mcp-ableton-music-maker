"""Sidechains in the Remote Script, against the model of what Live 12.4.6 does
(measured 2026-10-15; see fake_live: CompressorDevice, MaxDevice, DeviceIO,
RoutingType.attached_object, the track outputs, delete_track).

The requirements these hold (docs/: "requirements for sidechain support"):
R1 read the state; R2 set it in one call, read back, refuse what cannot be;
R3 which devices; R4/R5 effects loaded and reused; R6 a trigger's output;
R7 what deleting a source leaves, and the warnings; R9 inside a rack.
"""
import unittest

import harness
import fake_live
from test_duplex import connect
from test_every_command import Sink


def studio():
    """Kick (audio), Bass (MIDI + Operator), an empty MIDI track, a return."""
    song = fake_live.Song(scenes=2)
    song.create_audio_track()
    song.tracks[0].name = "Kick"
    song.create_midi_track()
    song.tracks[1].name = "Bass"
    song.create_midi_track()
    song.tracks[2].name = "Empty"
    song.create_return_track()
    browser = song._browser
    song.view.selected_track = song.tracks[1]
    browser.load_item(browser._find("query:Synths#Operator"))
    return song


class Base(unittest.TestCase):
    def setUp(self):
        self.ns = harness.load(song=studio())
        self.ns["SOCKET_READER"] = "main_thread_tick"
        self.script = harness.instance(self.ns)
        self.song = self.script.song()
        self.sock, self.client = connect(self.script, self.ns)

    def command(self, name, params=None, limit=400):
        sink = Sink()
        self.script._run_on_main(name, params or {}, sink)
        for _ in range(limit):
            if sink.payload is not None:
                break
            self.script.tick()
        self.assertIsNotNone(sink.payload, "%s never answered" % name)
        return sink.payload

    def ok(self, name, params=None):
        reply = self.command(name, params)
        self.assertEqual(reply.get("status"), "success", reply)
        return reply["result"]

    def refused(self, name, params, words):
        reply = self.command(name, params)
        self.assertEqual(reply.get("status"), "error", reply)
        self.assertIn(words, reply["message"])
        return reply["message"]

    def load(self, track, uri, name):
        return self.ok("ensure_effects", {"kind": "track", "index": track, "on_existing": "add",
                                          "effects": [{"uri": uri, "name": name}]})["effects"][0]["index"]

    def compressor_on_bass(self):
        i = self.load(1, "query:AudioFx#Compressor", "Compressor")
        return "song.tracks[1].devices[%d]" % i, self.song.tracks[1].devices[i]


class Sources(Base):
    """What a Compressor's sidechain offers, as Live 12.4.6 listed it."""

    def test_every_audio_track_its_own_too_the_returns_main_and_no_input(self):
        _, comp = self.compressor_on_bass()
        names = [t.display_name for t in comp.available_input_routing_types]
        self.assertEqual(names, ["Kick", "Bass", "A-Return", "Main", "No Input"])
        self.assertNotIn("Empty", names, "a MIDI track with no instrument is not offered")

    def test_a_track_source_has_three_taps(self):
        path, comp = self.compressor_on_bass()
        self.ok("set_sidechain", {"path": path, "source": {"kind": "track", "index": 0}})
        self.assertEqual([c.display_name for c in comp.available_input_routing_channels],
                         ["Pre FX", "Post FX", "Post Mixer"])


class SetSidechain(Base):
    """R2."""

    def test_each_tap_is_set_and_read_back(self):
        path, comp = self.compressor_on_bass()
        for tap, shown in (("pre_fx", "Pre FX"), ("post_fx", "Post FX"), ("post_mixer", "Post Mixer")):
            r = self.ok("set_sidechain", {"path": path, "source": {"kind": "track", "index": 0},
                                          "tap": tap, "params": {"on": 1.0}})
            self.assertEqual((r["sidechain"]["source"], r["sidechain"]["tap"], r["sidechain"]["tap_display"]),
                             ("Kick", tap, shown))
            self.assertTrue(r["sidechain"]["on"])
            self.assertIs(comp.input_routing_type.attached_object, self.song.tracks[0])

    def test_display_units_go_through_lives_own_display(self):
        path, comp = self.compressor_on_bass()
        r = self.ok("set_sidechain", {"path": path, "source": {"kind": "track", "index": 0},
                                      "displays": {"gain": "6 dB", "mix": "50 %", "eq_type": "Bell"},
                                      "params": {"eq_on": 0.0}})
        sc = r["sidechain"]
        self.assertEqual(sc["gain_display"], "6.00 dB")
        # Live shows S/C Mix in whole percent, so "50 %" lands within half a step.
        self.assertAlmostEqual(sc["mix"], 0.5, delta=0.006)
        self.assertEqual(sc["eq_type_display"], "Bell")
        self.assertFalse(sc["eq_on"])

    def test_its_own_track_is_refused_and_nothing_changes(self):
        path, comp = self.compressor_on_bass()
        self.refused("set_sidechain", {"path": path, "source": {"kind": "track", "index": 1}},
                     "cannot key from its own track, 'Bass'")
        self.assertEqual(comp.input_routing_type.display_name, "No Input")

    def test_a_bad_tap_is_refused_before_the_source_moves(self):
        path, comp = self.compressor_on_bass()
        self.refused("set_sidechain", {"path": path, "source": {"kind": "track", "index": 0}, "tap": "side"},
                     "tap must be pre_fx, post_fx or post_mixer")
        self.assertEqual(comp.input_routing_type.display_name, "No Input")

    def test_a_source_live_does_not_offer_is_refused(self):
        path, comp = self.compressor_on_bass()
        self.refused("set_sidechain", {"path": path, "source": {"kind": "track", "index": 2}},
                     "Live does not offer 'Empty'")

    def test_a_device_without_a_routable_input_is_refused_with_the_reason(self):
        i = self.load(1, "query:AudioFx#Glue%20Compressor", "Glue Compressor")
        self.refused("set_sidechain", {"path": "song.tracks[1].devices[%d]" % i,
                                       "source": {"kind": "track", "index": 0}},
                     "no sidechain input Live's API can route")

    def test_undo_puts_back_what_the_last_call_replaced(self):
        path, comp = self.compressor_on_bass()
        self.ok("set_sidechain", {"path": path, "source": {"kind": "track", "index": 0}, "tap": "pre_fx",
                                  "params": {"on": 1.0}})
        self.ok("set_sidechain", {"path": path, "source": {"kind": "return", "index": 0}, "tap": "post_mixer"})
        r = self.ok("set_sidechain", {"path": path, "undo": True})
        self.assertTrue(r["undone"])
        self.assertEqual((r["sidechain"]["source"], r["sidechain"]["tap"]), ("Kick", "pre_fx"))
        self.refused("set_sidechain", {"path": path, "undo": True, "source": None}, "nothing to undo")


class MaxDeviceKeys(Base):
    """A Max device's second audio input is its key (plugin~ 1 2 3 4)."""

    def test_amm_sidechain_keys_from_its_second_input(self):
        i = self.load(1, "query:UserLibrary#AbletonMusicMaker%20Devices:AMM%20Sidechain.amxd", "AMM Sidechain")
        dev = self.song.tracks[1].devices[i]
        r = self.ok("set_sidechain", {"path": "song.tracks[1].devices[%d]" % i,
                                      "source": {"kind": "track", "index": 0}, "tap": "post_fx"})
        self.assertIs(dev.audio_inputs[1].routing_type.attached_object, self.song.tracks[0])
        self.assertEqual(dev.audio_inputs[0].routing_type.display_name, "No Input")
        self.assertEqual(r["sidechain"]["amm"]["mode"], "duck")
        self.assertTrue(r["sidechain"]["on"])

    def test_modulate_holds_its_target_and_lets_go(self):
        glue = self.load(1, "query:AudioFx#Glue%20Compressor", "Glue Compressor")
        i = self.load(1, "query:UserLibrary#AbletonMusicMaker%20Devices:AMM%20Sidechain.amxd", "AMM Sidechain")
        amm = self.song.tracks[1].devices[i]
        threshold = self.song.tracks[1].devices[glue]._find_parameter("Threshold")
        for name, value in (("Target Device", glue), ("Target Param", 1), ("Mode", 2)):
            amm._find_parameter(name).value = value
        self.assertEqual(threshold.state, 2)
        self.assertFalse(threshold.is_enabled)
        state = self.ok("get_sidechain", {"path": "song.tracks[1].devices[%d]" % i})["sidechain"]["amm"]
        self.assertEqual(state["target"], {"device": "Glue Compressor", "parameter": "Threshold", "linked": True})
        amm._find_parameter("Mode").value = 0
        self.assertEqual(threshold.state, 0)


class Outputs(Base):
    """R6: a trigger nobody hears."""

    def test_sends_only_and_main_are_read_back(self):
        r = self.ok("set_track_output", {"track_index": 0, "to": "sends_only"})
        self.assertEqual((r["output"], r["previous"]["output"]), ("Sends Only", "Main"))
        r = self.ok("set_track_output", {"track_index": 0, "to": "main"})
        self.assertEqual(r["output"], "Main")

    def test_a_track_can_go_to_another_track(self):
        r = self.ok("set_track_output", {"track_index": 0, "to": {"kind": "track", "index": 1}})
        self.assertEqual(r["output_track"]["name"], "Bass")

    def test_create_tracks_takes_an_output(self):
        r = self.ok("create_tracks", {"tracks": [{"name": "Trigger", "kind": "audio", "output": "sends_only"}]})
        self.assertEqual(r["created"][0]["output"], "Sends Only")


class ContextAndWarnings(Base):
    """R1 and R7."""

    def test_get_context_lists_each_sidechain_and_the_output(self):
        path, comp = self.compressor_on_bass()
        self.ok("set_sidechain", {"path": path, "source": {"kind": "track", "index": 0}, "params": {"on": 1.0}})
        self.ok("set_track_output", {"track_index": 0, "to": "sends_only"})
        ctx = self.ok("get_context")
        bass = ctx["tracks"][1]
        self.assertEqual([(e["device"], e["sidechain"]["source"], e["sidechain"]["tap"]) for e in bass["sidechains"]],
                         [("Compressor", "Kick", "post_fx")])
        self.assertEqual(bass["devices"][1]["sidechain"]["source"], "Kick")
        self.assertEqual(ctx["tracks"][0]["output"], "Sends Only")
        self.assertEqual(ctx["warnings"], [])

    def test_deleting_the_source_names_what_it_unkeyed_and_warns(self):
        path, comp = self.compressor_on_bass()
        self.ok("set_sidechain", {"path": path, "source": {"kind": "track", "index": 0}, "params": {"on": 1.0}})
        r = self.ok("delete_track", {"track_index": 0})
        self.assertEqual(r["unkeyed"], [{"track": "Bass", "device": "Compressor", "in_chain": None,
                                         "tap": "post_fx", "now": "No Input", "on": True}])
        self.assertEqual(comp.input_routing_type.display_name, "No Input")
        warnings = self.ok("get_context")["warnings"]
        self.assertEqual([w["problem"] for w in warnings], ["no_source"])

    def test_renaming_the_source_keeps_the_routing(self):
        path, comp = self.compressor_on_bass()
        self.ok("set_sidechain", {"path": path, "source": {"kind": "track", "index": 0}})
        self.song.tracks[0].name = "Kick A"
        r = self.ok("get_sidechain", {"path": path})
        self.assertEqual(r["sidechain"]["source"], "Kick A")

    def test_a_post_mixer_key_from_a_muted_track_is_a_warning(self):
        path, comp = self.compressor_on_bass()
        self.ok("set_sidechain", {"path": path, "source": {"kind": "track", "index": 0}, "tap": "post_mixer",
                                  "params": {"on": 1.0}})
        self.song.tracks[0].mute = True
        warnings = self.ok("get_context")["warnings"]
        self.assertEqual([(w["problem"], w["source"]) for w in warnings], [("post_mixer_source_muted", "Kick")])

    def test_modulate_with_no_target_is_a_warning(self):
        i = self.load(1, "query:UserLibrary#AbletonMusicMaker%20Devices:AMM%20Sidechain.amxd", "AMM Sidechain")
        self.song.tracks[1].devices[i]._find_parameter("Mode").value = 2
        self.ok("set_sidechain", {"path": "song.tracks[1].devices[%d]" % i, "source": {"kind": "track", "index": 0}})
        self.assertIn("amm_no_target", [w["problem"] for w in self.ok("get_context")["warnings"]])


class Racks(Base):
    """R9: a Compressor inside a rack, by its chain."""

    def test_a_compressor_in_a_chain_is_listed_by_its_chain_and_keyed(self):
        comp = fake_live.CompressorDevice(self.song)
        chain = fake_live.Chain("Duck", self.song, [comp])
        rack = fake_live.Device("Audio Effect Rack", "AudioEffectGroupDevice", device_type=fake_live.TYPE_AUDIO_EFFECT,
                                chains=[chain])
        rack._canonical_parent = self.song.tracks[1]
        comp._canonical_parent = chain
        self.song.tracks[1]._devices.append(rack)
        listed = self.ok("list_sidechains", {"kind": "track", "index": 1})["sidechains"]
        self.assertEqual([(e["in_chain"], e["path"]) for e in listed],
                         [("Duck", "song.tracks[1].devices[1].chains[0].devices[0]")])
        self.ok("set_sidechain", {"path": listed[0]["path"], "source": {"kind": "track", "index": 0}})
        self.assertIs(comp.input_routing_type.attached_object, self.song.tracks[0])


class Effects(Base):
    """R4: loaded in order, reused when converging; parameters by name."""

    def test_converge_reuses_the_kth_device_of_a_name(self):
        spec = {"kind": "track", "index": 1, "on_existing": "converge",
                "effects": [{"uri": "query:AudioFx#Compressor", "name": "Compressor"},
                            {"uri": "query:AudioFx#Compressor", "name": "Compressor"}]}
        first = self.ok("ensure_effects", spec)["effects"]
        again = self.ok("ensure_effects", spec)["effects"]
        self.assertEqual([e["reused"] for e in first], [False, False])
        self.assertEqual([e["reused"] for e in again], [True, True])
        self.assertEqual([d.class_name for d in self.song.tracks[1].devices], ["Operator", "Compressor2", "Compressor2"])

    def test_parameters_by_name_in_display_units_and_held_ones_skipped(self):
        path, comp = self.compressor_on_bass()
        self.ok("set_device_parameters", {"track_index": 1, "device_index": 1, "values": [
            {"name": "Ratio", "value_display": "8 : 1"}, {"name": "threshold", "value": 0.5}]})
        self.assertEqual(comp._find_parameter("Ratio").str_for_value(comp._find_parameter("Ratio").value), "8.00 : 1")
        self.assertEqual(comp._find_parameter("Threshold").value, 0.5)
        self.refused("set_device_parameters", {"track_index": 1, "device_index": 1,
                                               "values": [{"name": "Nope", "value": 1}]}, "has no parameter 'Nope'")
        # A parameter an AMM Sidechain holds is skipped and said, not failed.
        i = self.load(1, "query:UserLibrary#AbletonMusicMaker%20Devices:AMM%20Sidechain.amxd", "AMM Sidechain")
        amm = self.song.tracks[1].devices[i]
        for name, value in (("Target Device", 1), ("Target Param", 1), ("Mode", 2)):
            amm._find_parameter(name).value = value
        r = self.ok("set_device_parameters", {"track_index": 1, "device_index": 1,
                                              "values": [{"name": "Threshold", "value": 0.2}]})
        self.assertEqual(str(r).count("held"), 1, r)
        self.assertEqual(comp._find_parameter("Threshold").value, 0.5)

    def test_an_export_reads_audio_effects_parameters_only(self):
        self.compressor_on_bass()
        snap = self.ok("get_session_snapshot", {"include_notes": False, "include_params": "effects"})
        devices = snap["tracks"][1]["devices"]
        self.assertEqual([(d["name"], d["type_code"], "parameters" in d) for d in devices],
                         [("Operator", 1, False), ("Compressor", 2, True)])
        self.assertEqual(devices[1]["sidechain"]["source"], None)


class Ratios(Base):
    """Compressor's Ratio shows "4.00 : 1" and tops out at "inf : 1"."""

    def test_ratio_displays_are_read_as_ratios(self):
        parts = self.script._display_parts
        self.assertEqual(parts("8 : 1"), (8.0, "ratio"))
        self.assertEqual(parts("8:1"), (8.0, "ratio"))
        self.assertEqual(parts("inf : 1"), (1e9, "ratio"))
        self.assertEqual(parts("1 : 1.50"), (1.5, "ratio"))
        path, comp = self.compressor_on_bass()
        ratio = comp._find_parameter("Ratio")
        self.assertEqual(ratio.str_for_value(self.script._value_from_display(ratio, "8 : 1")), "8.00 : 1")


if __name__ == "__main__":
    unittest.main()
