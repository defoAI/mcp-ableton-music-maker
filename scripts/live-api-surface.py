"""The Live API surface the Remote Script touches, read out of it by AST.

Every attribute read, attribute write and method call on a Live object,
grouped by the Live class the receiver is: the inventory the fake Live
Object Model (tests/remote_script/fake_live.py) is built and checked
against. Receivers are classified by name (`track`, `clip`, `slot`, ...)
and by what they hang off (`.tracks[i]` is a Track, `.mixer_device.volume`
a DeviceParameter); `hasattr`/`getattr`/`setattr` with a literal name count.

    python3 scripts/live-api-surface.py            # print the inventory
    python3 scripts/live-api-surface.py --json     # as JSON, for a test

Dict and str methods that share a variable name with a Live object
(`p.get`, `n.lower`) are filtered by NOISE; add to it when a false
positive appears, never to the model.
"""
import ast, collections, os, sys, re

SCRIPT = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                      "AbletonMusicMaker_Remote_Script", "__init__.py")
with open(SCRIPT, encoding="utf-8") as _handle:
    src = _handle.read()
tree = ast.parse(src)

# receiver name -> Live class
NAMES = {
    "song": "Song", "_song": "Song",
    "track": "Track", "tr": "Track", "t": "Track", "new_track": "Track",
    "return_track": "Track", "master": "Track", "master_track": "Track",
    "clip": "Clip", "c_clip": "Clip", "source": "Clip", "new_clip": "Clip",
    "slot": "ClipSlot", "clip_slot": "ClipSlot", "cs": "ClipSlot",
    "device": "Device", "dev": "Device", "d": "Device",
    "param": "DeviceParameter", "parameter": "DeviceParameter", "p": "DeviceParameter",
    "scene": "Scene", "sc": "Scene",
    "browser": "Browser", "item": "BrowserItem", "child": "BrowserItem",
    "app": "Application", "application": "Application",
    "envelope": "AutomationEnvelope", "env": "AutomationEnvelope",
    "mixer": "MixerDevice", "mixer_device": "MixerDevice",
    "note": "Note", "n": "Note",
    "cue": "CuePoint", "cue_point": "CuePoint",
    "view": "View",
}

hits = collections.defaultdict(lambda: collections.defaultdict(set))  # cls -> member -> {"get","call"}
unknown = collections.Counter()

def receiver(node):
    """Best-effort Live class of an attribute's receiver."""
    if isinstance(node, ast.Name):
        return NAMES.get(node.id)
    if isinstance(node, ast.Attribute):
        a = node.attr
        if a in ("_song",): return "Song"
        if a in ("tracks", "return_tracks"): return None   # a list
        if a == "master_track": return "Track"
        if a == "mixer_device": return "MixerDevice"
        if a == "clip": return "Clip"
        if a == "browser": return "Browser"
        if a == "view": return "View"
        if a in ("volume", "panning", "crossfader"): return "DeviceParameter"
        if isinstance(node.value, ast.Name) and node.value.id == "self" and a.startswith("_"):
            return None
        return None
    if isinstance(node, ast.Subscript):
        v = node.value
        if isinstance(v, ast.Attribute):
            if v.attr == "tracks" or v.attr == "return_tracks": return "Track"
            if v.attr == "clip_slots": return "ClipSlot"
            if v.attr == "devices": return "Device"
            if v.attr == "parameters": return "DeviceParameter"
            if v.attr == "scenes": return "Scene"
            if v.attr == "sends": return "DeviceParameter"
            if v.attr == "cue_points": return "CuePoint"
            if v.attr == "arrangement_clips": return "Clip"
    return None

class V(ast.NodeVisitor):
    def visit_Call(self, node):
        f = node.func
        if isinstance(f, ast.Attribute):
            cls = receiver(f.value)
            if cls:
                hits[cls][f.attr].add("call")
        # getattr(obj, "name") / hasattr(obj, "name")
        if isinstance(f, ast.Name) and f.id in ("getattr", "hasattr") and len(node.args) >= 2:
            cls = receiver(node.args[0])
            if isinstance(node.args[1], ast.Constant) and isinstance(node.args[1].value, str):
                if cls:
                    hits[cls][node.args[1].value].add("get")
        self.generic_visit(node)

    def visit_Attribute(self, node):
        cls = receiver(node.value)
        if cls:
            hits[cls][node.attr].add("get")
        self.generic_visit(node)

V().visit(tree)

# attribute writes
class W(ast.NodeVisitor):
    def visit_Assign(self, node):
        for tgt in node.targets:
            if isinstance(tgt, ast.Attribute):
                cls = receiver(tgt.value)
                if cls:
                    hits[cls][tgt.attr].add("set")
        self.generic_visit(node)
    def visit_AugAssign(self, node):
        if isinstance(node.target, ast.Attribute):
            cls = receiver(node.target.value)
            if cls: hits[cls][node.target.attr].add("set")
        self.generic_visit(node)
W().visit(tree)

# setattr(obj, "name", v)
for node in ast.walk(tree):
    if isinstance(node, ast.Call) and isinstance(node.func, ast.Name) and node.func.id == "setattr" and len(node.args) >= 2:
        cls = receiver(node.args[0])
        if isinstance(node.args[1], ast.Constant) and isinstance(node.args[1].value, str) and cls:
            hits[cls][node.args[1].value].add("set")

NOISE = {"get", "items", "lower", "startswith", "result", "is_alive", "keys", "values",
         "append", "strip", "split", "join", "replace", "format", "upper", "endswith"}
for cls in list(hits):
    for m in list(hits[cls]):
        if m in NOISE:
            del hits[cls][m]
    if not hits[cls]:
        del hits[cls]


def surface():
    """{class: {member: sorted kinds}} with kinds from get, set, call."""
    return {cls: {m: sorted(kinds) for m, kinds in members.items()}
            for cls, members in hits.items()}


if "--json" in sys.argv:
    import json
    print(json.dumps(surface(), indent=1, sort_keys=True))
    sys.exit(0)

total = 0
for cls in sorted(hits):
    members = hits[cls]
    total += len(members)
    print("=== %s (%d)" % (cls, len(members)))
    for m in sorted(members):
        print("    %-34s %s" % (m, ",".join(sorted(members[m]))))
print("\nTOTAL members:", total)
