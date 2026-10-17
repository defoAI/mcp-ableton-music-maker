#!/usr/bin/env python3
"""Build AMM Sidechain.amxd, the Max for Live device that gives any track a
sidechain the Live API can route.

Why it exists: Live's API routes a sidechain into exactly one built-in
device, Compressor (CompressorDevice.input_routing_type). Glue Compressor,
Gate, Auto Filter, Multiband Dynamics, Drum Buss and every plug-in expose no
routing to the API (Cycling '74 LOM reference; measured on Live 12.4.6,
2026-10-15). A Max for Live device's audio inputs, though, are DeviceIO
objects whose routing_type the API can set. This device is declared with
`plugin~ 1 2 3 4`: Live gives it two audio inputs and the second feeds
channels 3 and 4 -- the key.

Three modes, all measured on Live 12.4.6 against a kick:
  Duck     -- keyed gain reduction on its own track (threshold, ratio, depth,
              attack, release), optionally only below `Split` Hz through a
              Linkwitz-Riley crossover whose bands sum flat; Mix.
  Gate     -- the track opens while the key is over the threshold.
  Modulate -- the key moves one parameter of another device on the same track
              or rack chain (Target Device, Target Param, From and To as % of
              its range) through live.remote~, which writes no undo history
              and lets go when the mode changes.

The file is generated so that it can be read, reviewed and diffed:
`scripts/build-sidechain-device.py` writes AbletonMusicMaker_Devices/AMM
Sidechain.amxd, and tests/remote_script/test_sidechain_device.py fails when
the checked-in file is not what this script builds. AMM_DEBUG=1 adds
read-out parameters along the target path, for debugging against a Live.
"""
import json
import os
import struct
import sys

DEBUG = bool(os.environ.get("AMM_DEBUG"))
HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "..", "AbletonMusicMaker_Devices", "AMM Sidechain.amxd")

def box(i, text, nin, nout, otype=None, x=20, y=20, cls="newobj", w=120):
    b={"id":"obj-%d"%i,"maxclass":cls,"numinlets":nin,"numoutlets":nout,"patching_rect":[x,y,w,22.0]}
    if text is not None: b["text"]=text
    if otype is not None: b["outlettype"]=otype
    return {"box":b}
def line(src, so, dst, di): return {"patchline":{"source":["obj-%d"%src,so],"destination":["obj-%d"%dst,di]}}
def patcher(boxes, lines, title):
    return {"patcher":{"fileversion":1,"appversion":{"major":8,"minor":6,"revision":5,"architecture":"x64","modernui":1},
      "classnamespace":"box","rect":[34.0,79.0,640.0,480.0],"openrect":[0.0,0.0,0.0,169.0],"bglocked":0,"openinpresentation":0,
      "default_fontsize":10.0,"default_fontface":0,"default_fontname":"Arial Bold","gridonopen":1,"gridsize":[8.0,8.0],"gridsnaponopen":1,
      "objectsnaponopen":1,"statusbarvisible":2,"toolbarvisible":1,"devicewidth":0.0,"description":"","digest":"","tags":"","style":"",
      "subpatcher_template":"","title":title,"dependency_cache":[],"latency":0,
      "project":{"version":1,"creationdate":3590052493,"modificationdate":3590052493,"viewrect":[0.0,0.0,300.0,500.0],"autoorganize":1,
        "hideprojectwindow":1,"showdependencies":1,"autolocalize":0,"contents":{"patchers":{}},"layout":{},"searchpath":{},"detailsvisible":0,
        "amxdtype":1633771873,"readonly":0,"devpathtype":0,"devpath":".","sortmode":0},"autosave":0,
      "boxes":boxes,"lines":lines}}
def write(path, pat):
    body=(json.dumps(pat, indent=1)+"\n").encode()+b"\x00"
    data=b"ampf"+struct.pack("<I",4)+b"aaaa"+b"meta"+struct.pack("<I",4)+struct.pack("<I",0)+b"ptch"+struct.pack("<I",len(body))+body
    open(path,"wb").write(data)


GEN_CODE = r"""
// AMM Sidechain: in1/in2 the track, in3/in4 the key. out1/out2 the track;
// out3 the modulation, 0..1 of the target parameter's range (Modulate mode).
Param mode(0);        // 0 duck, 1 gate, 2 modulate
Param modfrom(0);     // %: where the target sits with no key
Param modto(100);     // %: where a full key takes it
Param threshold(-30); // dB, against the key's envelope
Param ratio(4);       // duck: dB over threshold -> dB of reduction
Param depth(24);      // duck: the most it reduces; gate: how far it closes, dB
Param attack(1);      // ms
Param release(120);   // ms
Param split(0);       // Hz: duck only below it; 0 = the whole band
Param mixpct(100);    // %

History env(0);
History gr(0);
History l1x1(0); History l1x2(0); History l1y1(0); History l1y2(0);
History l2x1(0); History l2x2(0); History l2y1(0); History l2y2(0);
History r1x1(0); History r1x2(0); History r1y1(0); History r1y2(0);
History r2x1(0); History r2x2(0); History r2y1(0); History r2y2(0);
History h1x1(0); History h1x2(0); History h1y1(0); History h1y2(0);
History h2x1(0); History h2x2(0); History h2y1(0); History h2y2(0);
History g1x1(0); History g1x2(0); History g1y1(0); History g1y2(0);
History g2x1(0); History g2x2(0); History g2y1(0); History g2y2(0);

key = max(abs(in3), abs(in4));
aa = exp(-1 / max(attack * 0.001 * samplerate, 1));
ra = exp(-1 / max(release * 0.001 * samplerate, 1));
env = key > env ? aa * env + (1 - aa) * key : ra * env + (1 - ra) * key;
lvl = atodb(max(env, 0.000001));

target = 0;
if (mode < 0.5) {
    over = max(lvl - threshold, 0);
    target = min(over * (1 - 1 / max(ratio, 1)), depth);
} else if (mode < 1.5) {
    target = lvl > threshold ? 0 : depth;
} else {
    // Modulate: how far the key rises over the threshold, up to depth dB.
    target = clamp(lvl - threshold, 0, max(depth, 0.001));
}
// Reduction rises at the attack rate and falls back at the release rate.
gr = target > gr ? aa * gr + (1 - aa) * target : ra * gr + (1 - ra) * target;
g = mode < 1.5 ? dbtoa(-gr) : 1;
m = mode > 1.5 ? gr / max(depth, 0.001) : 0;

// Split: a 4th-order Linkwitz-Riley crossover (each band two cascaded
// 2nd-order Butterworth sections). The low and high bands sum flat in level,
// so with split on, only the band below `split` is ducked and nothing above
// it changes.
f = clamp(split, 20, samplerate * 0.45);
w = tan(pi * f / samplerate);
n = 1 / (1 + sqrt(2) * w + w * w);
a1 = 2 * (w * w - 1) * n; a2 = (1 - sqrt(2) * w + w * w) * n;
lb0 = w * w * n; lb1 = 2 * lb0;
hb0 = n; hb1 = -2 * n;

y = lb0 * in1 + lb1 * l1x1 + lb0 * l1x2 - a1 * l1y1 - a2 * l1y2;
l1x2 = l1x1; l1x1 = in1; l1y2 = l1y1; l1y1 = y;
lowL = lb0 * y + lb1 * l2x1 + lb0 * l2x2 - a1 * l2y1 - a2 * l2y2;
l2x2 = l2x1; l2x1 = y; l2y2 = l2y1; l2y1 = lowL;
y = hb0 * in1 + hb1 * h1x1 + hb0 * h1x2 - a1 * h1y1 - a2 * h1y2;
h1x2 = h1x1; h1x1 = in1; h1y2 = h1y1; h1y1 = y;
highL = hb0 * y + hb1 * h2x1 + hb0 * h2x2 - a1 * h2y1 - a2 * h2y2;
h2x2 = h2x1; h2x1 = y; h2y2 = h2y1; h2y1 = highL;

y = lb0 * in2 + lb1 * r1x1 + lb0 * r1x2 - a1 * r1y1 - a2 * r1y2;
r1x2 = r1x1; r1x1 = in2; r1y2 = r1y1; r1y1 = y;
lowR = lb0 * y + lb1 * r2x1 + lb0 * r2x2 - a1 * r2y1 - a2 * r2y2;
r2x2 = r2x1; r2x1 = y; r2y2 = r2y1; r2y1 = lowR;
y = hb0 * in2 + hb1 * g1x1 + hb0 * g1x2 - a1 * g1y1 - a2 * g1y2;
g1x2 = g1x1; g1x1 = in2; g1y2 = g1y1; g1y1 = y;
highR = hb0 * y + hb1 * g2x1 + hb0 * g2x2 - a1 * g2y1 - a2 * g2y2;
g2x2 = g2x1; g2x1 = y; g2y2 = g2y1; g2y1 = highR;

wetL = split > 0 ? lowL * g + highL : in1 * g;
wetR = split > 0 ? lowR * g + highR : in2 * g;
mix = clamp(mixpct * 0.01, 0, 1);
out1 = in1 * (1 - mix) + wetL * mix;
out2 = in2 * (1 - mix) + wetR * mix;
out3 = clamp((modfrom + (modto - modfrom) * m) * 0.01, 0, 1);
"""

def gen_box(i, x, y):
    boxes = [
        {"box": {"id": "obj-c", "maxclass": "codebox", "code": GEN_CODE, "fontface": 0, "fontname": "<Monospaced>",
                 "fontsize": 12.0, "numinlets": 4, "numoutlets": 3, "outlettype": ["", "", ""],
                 "patching_rect": [20.0, 60.0, 600.0, 400.0]}},
    ]
    lines = []
    for k in range(4):
        boxes.append({"box": {"id": "obj-i%d" % (k + 1), "maxclass": "newobj", "text": "in %d" % (k + 1),
                              "numinlets": 0, "numoutlets": 1, "outlettype": [""], "patching_rect": [20.0 + 60 * k, 20.0, 40.0, 22.0]}})
        lines.append({"patchline": {"source": ["obj-i%d" % (k + 1), 0], "destination": ["obj-c", k]}})
    for k in range(3):
        boxes.append({"box": {"id": "obj-o%d" % (k + 1), "maxclass": "newobj", "text": "out %d" % (k + 1),
                              "numinlets": 1, "numoutlets": 0, "patching_rect": [20.0 + 60 * k, 480.0, 40.0, 22.0]}})
        lines.append({"patchline": {"source": ["obj-c", k], "destination": ["obj-o%d" % (k + 1), 0]}})
    gp = {"fileversion": 1, "appversion": {"major": 9, "minor": 1, "revision": 5, "architecture": "x64", "modernui": 1},
          "classnamespace": "dsp.gen", "rect": [50.0, 50.0, 700.0, 560.0], "bglocked": 0, "openinpresentation": 0,
          "default_fontsize": 12.0, "default_fontface": 0, "default_fontname": "Arial", "gridonopen": 1, "gridsize": [15.0, 15.0],
          "gridsnaponopen": 1, "objectsnaponopen": 1, "statusbarvisible": 2, "toolbarvisible": 1, "boxes": boxes, "lines": lines}
    return {"box": {"id": "obj-%d" % i, "maxclass": "newobj", "text": "gen~", "numinlets": 4, "numoutlets": 3,
                    "outlettype": ["signal", "signal", "signal"], "patching_rect": [x, y, 200.0, 22.0], "patcher": gp}}

# name, gen param, min, max, initial, unitstyle (1 float, 2 time ms, 3 Hz, 4 dB, 5 %), enum
DIALS = [
    ("Mode", "mode", 0, 2, 0, None, ["Duck", "Gate", "Modulate"]),
    ("Threshold", "threshold", -60.0, 0.0, -30.0, 4, None),
    ("Ratio", "ratio", 1.0, 20.0, 4.0, 1, None),
    ("Depth", "depth", 0.0, 60.0, 24.0, 4, None),
    ("Attack", "attack", 0.1, 100.0, 1.0, 2, None),
    ("Release", "release", 5.0, 2000.0, 120.0, 2, None),
    ("Split", "split", 0.0, 500.0, 0.0, 3, None),
    ("Mix", "mixpct", 0.0, 100.0, 100.0, 5, None),
    ("From", "modfrom", 0.0, 100.0, 0.0, 5, None),
    ("To", "modto", 0.0, 100.0, 100.0, 5, None),
]
# The Modulate target, on the device's own track (or rack chain): device and
# parameter by position. Ints; -1 is none.
TARGETS = [("Target Device", -1, 127, -1), ("Target Param", 0, 1023, 0)]

def dial(i, name, lo, hi, init, unit, enum, x, y):
    vo = {"parameter_initial": [init], "parameter_initial_enable": 1, "parameter_longname": name,
          "parameter_shortname": name, "parameter_mmin": lo, "parameter_mmax": hi, "parameter_modmode": 0,
          "parameter_speedlim": 0.0}
    if enum:
        vo.update({"parameter_type": 2, "parameter_enum": enum, "parameter_mmax": len(enum) - 1})
        cls = "live.tab"
        b = {"id": "obj-%d" % i, "maxclass": "live.tab", "num_lines_patching": 1, "num_lines_presentation": 1,
             "numinlets": 1, "numoutlets": 3, "outlettype": ["", "", "float"], "parameter_enable": 1,
             "patching_rect": [x, y, 100.0, 20.0], "presentation": 1, "presentation_rect": [x, 4.0, 100.0, 20.0],
             "saved_attribute_attributes": {"valueof": vo}, "varname": name}
    else:
        vo.update({"parameter_type": 0, "parameter_unitstyle": unit})
        b = {"id": "obj-%d" % i, "maxclass": "live.dial", "numinlets": 1, "numoutlets": 2, "outlettype": ["", "float"],
             "parameter_enable": 1, "patching_rect": [x, y, 44.0, 48.0], "presentation": 1,
             "presentation_rect": [x, 30.0, 44.0, 48.0], "saved_attribute_attributes": {"valueof": vo}, "varname": name}
    return {"box": b}

def device():
    boxes, lines = [], []
    boxes.append(box(1, "plugin~ 1 2 3 4", 1, 4, ["signal"] * 4, 20, 20))
    boxes.append(gen_box(2, 20, 300))
    boxes.append(box(3, "plugout~ 1 2", 2, 0, None, 20, 400))
    for k in range(4):
        lines.append(line(1, k, 2, k))
    lines.append(line(2, 0, 3, 0))
    lines.append(line(2, 1, 3, 1))
    n = 10
    for k, (name, gp, lo, hi, init, unit, enum) in enumerate(DIALS):
        x = 20 + 60 * k
        boxes.append(dial(n, name, lo, hi, init, unit, enum, x, 100))
        boxes.append(box(n + 1, "prepend %s" % gp, 1, 1, [""], x, 180, w=60))
        lines.append(line(n, 0, n + 1, 0))
        lines.append(line(n + 1, 0, 2, 0))
        n += 2
    # Modulate: resolve the target relative to this device, and drive it.
    # live.path's left outlet keeps the id it found even if the object moves
    # in Live, so the target follows its parameter when tracks are inserted.
    def numbox(i, name, lo, hi, init, x, y):
        vo = {"parameter_initial": [init], "parameter_initial_enable": 1, "parameter_longname": name,
              "parameter_shortname": name, "parameter_mmin": lo, "parameter_mmax": hi,
              "parameter_type": 1, "parameter_unitstyle": 0, "parameter_modmode": 0, "parameter_speedlim": 0.0}
        return {"box": {"id": "obj-%d" % i, "maxclass": "live.numbox", "numinlets": 1, "numoutlets": 2,
                        "outlettype": ["", "float"], "parameter_enable": 1, "patching_rect": [x, y, 44.0, 15.0],
                        "presentation": 1, "presentation_rect": [x, 84.0, 44.0, 15.0],
                        "saved_attribute_attributes": {"valueof": vo}, "varname": name}}
    mode_tab = 10  # the first dial built above is Mode
    boxes.append(numbox(80, "Target Device", -1, 127, -1, 20, 220))
    boxes.append(numbox(81, "Target Param", 0, 1023, 0, 80, 220))
    boxes.append(box(82, "live.thisdevice", 1, 3, ["bang", "int", "int"], 200, 220))
    boxes.append(box(83, "pak 0 -1 0", 3, 1, [""], 20, 250))
    boxes.append(box(84, "if $i1 == 2 && $i2 >= 0 then $i2 $i3 else out2 bang", 3, 2, ["", ""], 20, 280, w=300))
    # The target's path is this device's own, with its last step ("devices N")
    # replaced: live.path cannot step from this_device to its parent (a
    # relative goto answered id 0 on Live 12.4.6), but getpath can say where
    # this device is. "live_set tracks 16 devices 2" -> "live_set tracks 16",
    # then "devices D parameters P" -- under a rack chain, the chain's own.
    boxes.append(box(85, "t b l", 1, 2, ["bang", ""], 20, 310))
    boxes.append({"box": {"id": "obj-89", "maxclass": "message", "text": "devices $1 parameters $2",
                          "numinlets": 2, "numoutlets": 1, "outlettype": [""], "patching_rect": [200.0, 340.0, 160.0, 22.0]}})
    boxes.append(box(70, "zl join", 2, 2, ["", ""], 20, 520))
    boxes.append({"box": {"id": "obj-71", "maxclass": "message", "text": "path this_device",
                          "numinlets": 2, "numoutlets": 1, "outlettype": [""], "patching_rect": [20.0, 340.0, 160.0, 22.0]}})
    boxes.append(box(72, "live.path", 1, 3, ["", "", ""], 20, 370))
    # live.path names this device by id; live.object's getpath says where it is.
    boxes.append(box(60, "t b l", 1, 2, ["bang", ""], 200, 370))
    boxes.append(box(61, "live.object", 2, 1, [""], 200, 390))
    boxes.append({"box": {"id": "obj-62", "maxclass": "message", "text": "getpath",
                          "numinlets": 2, "numoutlets": 1, "outlettype": [""], "patching_rect": [260.0, 370.0, 60.0, 22.0]}})
    boxes.append(box(73, "route path", 1, 2, ["", ""], 20, 400))
    # getpath answers the path as one quoted symbol, not a list of words:
    # made into a symbol whatever it is, then split.
    boxes.append(box(78, "tosymbol", 1, 1, [""], 200, 400))
    boxes.append(box(79, "fromsymbol", 1, 1, [""], 200, 415))
    boxes.append(box(74, "zl rev", 2, 2, ["", ""], 20, 430))
    boxes.append(box(75, "zl slice 2", 2, 2, ["", ""], 20, 460))
    boxes.append(box(76, "zl rev", 2, 2, ["", ""], 20, 490))
    boxes.append(box(77, "prepend path", 1, 1, [""], 20, 550))
    boxes.append(box(86, "live.path", 1, 3, ["", "", ""], 20, 580))
    boxes.append({"box": {"id": "obj-87", "maxclass": "message", "text": "id 0",
                          "numinlets": 2, "numoutlets": 1, "outlettype": [""], "patching_rect": [340.0, 310.0, 40.0, 22.0]}})
    boxes.append(box(88, "live.remote~ @normalized 1 @smoothing 1", 2, 0, None, 20, 620, w=240))
    lines.extend([line(mode_tab, 0, 83, 0), line(80, 0, 83, 1), line(81, 0, 83, 2), line(82, 0, 83, 0),
                  line(83, 0, 84, 0), line(84, 0, 85, 0),
                  line(85, 1, 89, 0), line(89, 0, 70, 1),
                  line(85, 0, 71, 0), line(71, 0, 72, 0), line(72, 0, 60, 0), line(60, 1, 61, 1), line(60, 0, 62, 0),
                  line(62, 0, 61, 0), line(61, 0, 73, 0), line(73, 0, 78, 0), line(78, 0, 79, 0), line(79, 0, 74, 0),
                  line(74, 0, 75, 0), line(75, 1, 76, 0), line(76, 0, 70, 0), line(70, 0, 77, 0),
                  line(77, 0, 86, 0), line(86, 0, 88, 1),
                  line(84, 1, 87, 0), line(87, 0, 88, 1), line(2, 2, 88, 0)])
    if DEBUG:
        boxes.append(numbox(90, "Dbg Id", -1, 1000000, -1, 400, 220))
        boxes.append(box(91, "route id", 1, 2, ["", ""], 400, 370))
        lines.extend([line(86, 0, 91, 0), line(91, 0, 90, 0)])
        boxes.append(numbox(92, "Dbg If", -1, 1000000, -1, 460, 220))
        boxes.append(box(93, "unpack 0 0", 1, 2, ["int", "int"], 460, 300))
        lines.extend([line(84, 0, 93, 0), line(93, 1, 92, 0)])
        boxes.append(numbox(94, "Dbg Ready", -1, 1000000, -1, 520, 220))
        boxes.append(box(95, "counter", 5, 4, ["int", "", "", "int"], 520, 260))
        lines.extend([line(82, 0, 95, 0), line(95, 0, 94, 0)])
        nid = 200
        for name, src, outlet in (("Dbg A0", 72, 0), ("Dbg A1", 72, 1), ("Dbg A2", 72, 2), ("Dbg Route", 73, 0),
                                  ("Dbg RouteX", 73, 1), ("Dbg Join", 70, 0), ("Dbg B0", 86, 0), ("Dbg Trig", 85, 0),
                                  ("Dbg Rev1", 74, 0), ("Dbg Obj", 61, 0), ("Dbg SliceL", 75, 0), ("Dbg SliceR", 75, 1), ("Dbg Rev2", 76, 0), ("Dbg Msg", 89, 0)):
            boxes.append(numbox(nid, name, -1, 1000000, -1, 640 + nid - 200, 220))
            boxes.append(box(nid + 1, "t b", 1, 1, ["bang"], 640 + nid - 200, 260))
            boxes.append(box(nid + 2, "counter", 5, 4, ["int", "", "", "int"], 640 + nid - 200, 300))
            lines.extend([line(src, outlet, nid + 1, 0), line(nid + 1, 0, nid + 2, 0), line(nid + 2, 0, nid, 0)])
            nid += 3
        boxes.append(numbox(300, "Dbg Len", -1, 1000, -1, 900, 220))
        boxes.append(box(301, "zl len", 2, 2, ["", ""], 900, 260))
        lines.extend([line(79, 0, 301, 0), line(301, 0, 300, 0)])
        boxes.append(numbox(96, "Dbg Else", -1, 1000000, -1, 580, 220))
        boxes.append(box(97, "counter", 5, 4, ["int", "", "", "int"], 580, 300))
        lines.extend([line(84, 1, 97, 0), line(97, 0, 96, 0)])
    pat = patcher(boxes, lines, "AMM Sidechain")
    pat["patcher"]["openinpresentation"] = 1
    pat["patcher"]["devicewidth"] = 500.0
    return pat

def build_bytes():
    """The device as bytes: an `ampf` header naming an audio effect
    ('aaaa'), an empty `meta` chunk and the patcher as JSON in `ptch` -- the
    layout of the uncompressed .amxd files Live ships."""
    body = (json.dumps(device(), indent=1, sort_keys=True) + "\n").encode("utf-8") + b"\x00"
    return (b"ampf" + struct.pack("<I", 4) + b"aaaa" + b"meta" + struct.pack("<I", 4)
            + struct.pack("<I", 0) + b"ptch" + struct.pack("<I", len(body)) + body)


if __name__ == "__main__":
    out = sys.argv[1] if len(sys.argv) > 1 else OUT
    with open(out, "wb") as handle:
        handle.write(build_bytes())
    print("wrote %s (%d bytes)" % (out, len(build_bytes())))
