"""The script declares no commands by hand.

`SCRIPT_CAPABILITIES` is read back from the body's own dispatch at import and
`LOADER_CAPABILITIES` from the loader's, by the same function, so a handler
that exists is declared and a name with no handler cannot be. What the script
answers is the two together, which is what `get_script_info` reports.

The server's `ALL_REMOTE_COMMANDS` (src/tools.rs) is the source of truth for
what may be asked for; the Rust test
`the_servers_command_list_and_the_scripts_dispatch_are_the_same_set` pins the
two together, reading both files. These tests cover the derivation itself.
"""
import os
import re
import unittest

import harness


SCRIPT = harness.SCRIPT
BODY = harness.BODY


def dispatched_names(path=None):
    """The dispatch's command names, found independently of the script's own
    regexes, so the two cannot be wrong in the same way."""
    with open(path or BODY) as handle:
        source = handle.read()
    names = set()
    for chunk in source.split('command_type == "')[1:]:
        names.add(chunk.split('"')[0])
    for chunk in source.split("command_type in (")[1:]:
        group = chunk.split(")")[0]
        names.update(re.findall(r'"([a-z_]+)"', group))
    return names


class Derivation(unittest.TestCase):
    def setUp(self):
        self.ns = harness.load()

    def test_the_capability_list_is_derived_and_not_empty(self):
        caps = self.ns["SCRIPT_CAPABILITIES"]
        self.assertTrue(caps, "the derivation produced nothing")
        self.assertGreater(len(caps), 50, "suspiciously few commands")

    def test_it_is_exactly_what_the_dispatch_handles(self):
        self.assertEqual(set(self.ns["SCRIPT_CAPABILITIES"]), dispatched_names(BODY))

    def test_the_loader_derives_its_four_the_same_way(self):
        self.assertEqual(set(self.ns["LOADER_CAPABILITIES"]), dispatched_names(SCRIPT))
        self.assertEqual(sorted(self.ns["LOADER_CAPABILITIES"]),
                         ["get_script_info", "reload_body", "subscribe", "unsubscribe"])

    def test_what_the_script_serves_is_the_two_lists_together(self):
        script = harness.instance(self.ns)
        served = script._capabilities()
        self.assertEqual(set(served),
                         set(self.ns["SCRIPT_CAPABILITIES"]) | set(self.ns["LOADER_CAPABILITIES"]))
        self.assertEqual(served, sorted(served))
        self.assertEqual(served, script._get_script_info()["capabilities"])

    def test_a_loader_with_no_body_still_declares_its_own(self):
        script = harness.instance(self.ns, body=False)
        self.assertEqual(script._capabilities(), sorted(self.ns["LOADER_CAPABILITIES"]))
        self.assertIn("reload_body", script._capabilities())

    def test_it_is_sorted_and_free_of_duplicates(self):
        caps = self.ns["SCRIPT_CAPABILITIES"]
        self.assertEqual(caps, sorted(caps))
        self.assertEqual(len(caps), len(set(caps)))

    def test_the_commands_reached_before_the_dispatch_are_the_loaders(self):
        # get_script_info answers ahead of the chain, subscribe and
        # unsubscribe are handled on the client drain, and reload_body is the
        # one the loader must answer even when the body cannot. All four are
        # commands, and all four are now the loader's.
        for name in ("get_script_info", "subscribe", "unsubscribe", "reload_body"):
            self.assertIn(name, self.ns["LOADER_CAPABILITIES"], name)
            self.assertNotIn(name, self.ns["SCRIPT_CAPABILITIES"], name)

    def test_a_name_with_no_handler_is_not_declared(self):
        self.assertNotIn("teleport_track", self.ns["SCRIPT_CAPABILITIES"])

    def test_the_deleted_commands_are_gone(self):
        # Handlers the server never asked for; deleted when the server became
        # the one place the command list lives.
        for name in ("get_browser_item", "get_browser_categories",
                     "get_browser_items", "inspect_rack",
                     "map_rack_magnitude", "load_instrument_or_effect"):
            self.assertNotIn(name, self.ns["SCRIPT_CAPABILITIES"], name)
            self.assertNotIn("_" + name, dir(self.ns["Body"]), name)

    def test_no_hand_written_list_has_crept_back(self):
        with open(BODY) as handle:
            self.assertNotIn("SCRIPT_CAPABILITIES = [", handle.read())
        with open(SCRIPT) as handle:
            self.assertNotIn("LOADER_CAPABILITIES = [", handle.read())

    def test_an_unreadable_file_gives_an_empty_list_not_an_exception(self):
        # The server falls back to its own list; the script must not raise
        # during import inside Live because a path moved.
        served = self.ns["_served_commands"]
        self.assertEqual(served(os.path.join(os.sep, "nowhere", "body.py")), [])

    # Four commands are answered before the dispatch chain is reached, and
    # all four are the loader's: get_script_info needs no main thread,
    # subscribe and unsubscribe belong to the socket that asked, and
    # reload_body must answer even when the body cannot. Everything else
    # must be known to _dispatch.
    BEFORE_DISPATCH = ("get_script_info", "subscribe", "unsubscribe", "reload_body")

    def test_no_declared_command_falls_through_to_unknown(self):
        script = harness.instance(self.ns)
        unknown = []
        for name in script._capabilities():
            if name in self.BEFORE_DISPATCH:
                continue
            try:
                script._dispatch(name, {}, None)
            except Exception as e:
                if "Unknown command" in str(e):
                    unknown.append(name)
        self.assertEqual(unknown, [], "declared but the dispatch rejects them")

    def test_the_commands_answered_before_the_dispatch_are_only_those_four(self):
        script = harness.instance(self.ns)
        early = []
        for name in self.BEFORE_DISPATCH:
            try:
                script._dispatch(name, {}, None)
            except Exception as e:
                if "Unknown command" in str(e):
                    early.append(name)
        self.assertEqual(sorted(early), sorted(self.BEFORE_DISPATCH),
                         "a command moved into or out of the dispatch chain")


class NoNameDefinedTwice(unittest.TestCase):
    """A method defined twice in one class is not an error in Python: the
    later one silently wins.

    That is how 1.33.0 shipped with `fire_clip`, `fire_scene`, `record_clip`
    and `start_live_capture` all raising TypeError inside Live. #47 added a
    `_landing(param, asked, clamped)` beside the `_landing()` that says which
    bar a launch lands on, and every zero-argument call started failing. No
    test saw it, because every test that called the new one got the new one
    and no test called the old one against a real Song.

    This is the cheap guard: one scan of the file, no name defined twice.
    """

    def check_no_name_twice(self, path):
        import ast
        with open(path) as handle:
            tree = ast.parse(handle.read(), path)
        clashes = []
        for node in ast.walk(tree):
            if not isinstance(node, ast.ClassDef):
                continue
            seen = {}
            for item in node.body:
                names = []
                if isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef)):
                    names = [item.name]
                elif isinstance(item, ast.Assign):
                    names = [t.id for t in item.targets if isinstance(t, ast.Name)]
                for name in names:
                    if name in seen:
                        clashes.append("%s.%s: line %d shadows line %d" % (
                            node.name, name, item.lineno, seen[name]))
                    seen[name] = item.lineno
        self.assertEqual(clashes, [], "a name is defined twice in one class:\n  " +
                         "\n  ".join(clashes))

    def test_the_loader_defines_no_name_twice_in_one_class(self):
        self.check_no_name_twice(SCRIPT)

    def test_the_body_defines_no_name_twice_in_one_class(self):
        self.check_no_name_twice(BODY)

    def check_no_module_level_name_twice(self, path):
        import ast
        with open(path) as handle:
            tree = ast.parse(handle.read(), path)
        seen, clashes = {}, []
        for item in tree.body:
            names = []
            if isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                names = [item.name]
            for name in names:
                if name in seen:
                    clashes.append("%s: line %d shadows line %d" % (name, item.lineno, seen[name]))
                seen[name] = item.lineno
        self.assertEqual(clashes, [])

    def test_no_module_level_name_is_defined_twice_either(self):
        self.check_no_module_level_name_twice(SCRIPT)
        self.check_no_module_level_name_twice(BODY)


if __name__ == "__main__":
    unittest.main()
