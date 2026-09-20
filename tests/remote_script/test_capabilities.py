"""The script declares no commands by hand.

`SCRIPT_CAPABILITIES` is read back from the script's own dispatch at import,
so a handler that exists is declared and a name with no handler cannot be.
The server's `ALL_REMOTE_COMMANDS` (src/tools.rs) is the source of truth for
what may be asked for; the Rust test
`the_servers_command_list_and_the_scripts_dispatch_are_the_same_set` pins the
two together. These tests cover the derivation itself.
"""
import os
import re
import unittest

import harness


SCRIPT = harness.SCRIPT


def dispatched_names():
    """The dispatch's command names, found independently of the script's own
    regexes, so the two cannot be wrong in the same way."""
    with open(SCRIPT) as handle:
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
        self.assertEqual(set(self.ns["SCRIPT_CAPABILITIES"]), dispatched_names())

    def test_it_is_sorted_and_free_of_duplicates(self):
        caps = self.ns["SCRIPT_CAPABILITIES"]
        self.assertEqual(caps, sorted(caps))
        self.assertEqual(len(caps), len(set(caps)))

    def test_the_commands_reached_before_the_dispatch_are_included(self):
        # get_script_info answers ahead of the chain; subscribe and
        # unsubscribe are handled on the client drain. All three are commands.
        for name in ("get_script_info", "subscribe", "unsubscribe"):
            self.assertIn(name, self.ns["SCRIPT_CAPABILITIES"], name)

    def test_a_name_with_no_handler_is_not_declared(self):
        self.assertNotIn("teleport_track", self.ns["SCRIPT_CAPABILITIES"])

    def test_the_deleted_commands_are_gone(self):
        # Handlers the server never asked for; deleted when the server became
        # the one place the command list lives.
        for name in ("get_browser_item", "get_browser_categories",
                     "get_browser_items", "inspect_rack",
                     "map_rack_magnitude", "load_instrument_or_effect"):
            self.assertNotIn(name, self.ns["SCRIPT_CAPABILITIES"], name)
            self.assertNotIn("_" + name, dir(self.ns["AbletonMCP"]), name)

    def test_no_hand_written_list_has_crept_back(self):
        with open(SCRIPT) as handle:
            self.assertNotIn("SCRIPT_CAPABILITIES = [", handle.read())

    def test_an_unreadable_file_gives_an_empty_list_not_an_exception(self):
        # The server falls back to its own list; the script must not raise
        # during import inside Live because a path moved.
        served = self.ns["_served_commands"]
        ns_file = self.ns["__file__"]
        try:
            self.ns["__file__"] = os.path.join(os.sep, "nowhere", "__init__.py")
            # Rebind the function's globals view of __file__.
            served.__globals__["__file__"] = self.ns["__file__"]
            self.assertEqual(served(), [])
        finally:
            served.__globals__["__file__"] = ns_file
            self.ns["__file__"] = ns_file

    # Three commands are answered before the dispatch chain is reached:
    # get_script_info needs no main thread, and subscribe/unsubscribe belong
    # to the socket that asked. Everything else must be known to _dispatch.
    BEFORE_DISPATCH = ("get_script_info", "subscribe", "unsubscribe")

    def test_no_declared_command_falls_through_to_unknown(self):
        script = harness.instance(self.ns)
        unknown = []
        for name in self.ns["SCRIPT_CAPABILITIES"]:
            if name in self.BEFORE_DISPATCH:
                continue
            try:
                script._dispatch(name, {}, None)
            except Exception as e:
                if "Unknown command" in str(e):
                    unknown.append(name)
        self.assertEqual(unknown, [], "declared but the dispatch rejects them")

    def test_the_commands_answered_before_the_dispatch_are_only_those_three(self):
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

    def test_the_script_defines_no_name_twice_in_one_class(self):
        import ast
        with open(SCRIPT) as handle:
            tree = ast.parse(handle.read(), SCRIPT)
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

    def test_no_module_level_name_is_defined_twice_either(self):
        import ast
        with open(SCRIPT) as handle:
            tree = ast.parse(handle.read(), SCRIPT)
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


if __name__ == "__main__":
    unittest.main()
