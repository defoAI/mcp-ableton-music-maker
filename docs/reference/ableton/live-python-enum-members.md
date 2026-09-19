# Live Python enum member names

| | |
|---|---|
| Source | Ableton's own control-surface scripts as shipped inside Live 12 (decompiled copies at https://github.com/gluon/AbletonLive12_MIDIRemoteScripts: `LV2_LX2_LC2_LD2/FaderfoxHelper.py`, `pushbase/fixed_length.py`, `pushbase/quantization_component.py`, `ableton/v3/control_surface/components/clip_actions.py`, `_MxDCore/LomTypes.py`); for `Live.Clip.LaunchMode` and `Live.Clip.ClipLaunchQuantization` only the third-party https://github.com/leolabs/ableton-js (`midi-script/Namespaces/Clip.py`, `src/ns/clip.ts`) |
| Fetched | 2026-09-19 |
| Live version documented | Live 12 (the scripts carry the embedded-file path `output/Live/mac_universal_64_static/Release/python-bundle/MIDI Remote Scripts/...`) |
| Copyright | Ableton AG for the script contents; this page is our compilation of names read from them, not an Ableton document. |

The Live Object Model ([live-object-model.md](live-object-model.md)) documents `clip_trigger_quantization`, `launch_mode` and `launch_quantization` as integers. Live's embedded Python exposes the same values as enum members on the `Live` module; the LOM does not list those names, so they are collected here from Ableton's shipped scripts. `_MxDCore/LomTypes.py` names `Live.Song.Quantization`, `Live.Song.RecordingQuantization`, `Live.Song.CaptureMode`, `Live.Clip.GridQuantization`, `Live.DeviceParameter.AutomationState`, `Live.Groove.Base`, `Live.Sample.SlicingStyle` and `Live.Sample.SlicingBeatDivision` as `ENUM_TYPES`.

## `Live.Song.Quantization` (Live 12, Ableton's `FaderfoxHelper.py`)

The int column is the LOM's `Song.clip_trigger_quantization` encoding; the beats column is the value `FaderfoxHelper.py` maps each member to.

| Member | LOM int | Beats (4/4) |
|---|---|---|
| `q_no_q` | 0 (None) | 0.03125 (used by the script as "none") |
| `q_8_bars` | 1 | 32.0 |
| `q_4_bars` | 2 | 16.0 |
| `q_2_bars` | 3 | 8.0 |
| `q_bar` | 4 | 4.0 |
| `q_half` | 5 | 2.0 |
| `q_half_triplet` | 6 | 1 + 1/3 |
| `q_quarter` | 7 | 1.0 |
| `q_quarter_triplet` | 8 | 2/3 |
| `q_eight` | 9 | 0.5 |
| `q_eight_triplet` | 10 | 1/3 |
| `q_sixtenth` | 11 | 0.25 |
| `q_sixtenth_triplet` | 12 | 1/6 |
| `q_thirtytwoth` | 13 | 0.125 |

Spellings are Ableton's: `q_eight` (not `q_eighth`), `q_sixtenth` (not `q_sixteenth`), `q_thirtytwoth` (not `q_thirtysecond`).

`pushbase/fixed_length.py` passes these members as the `launch_quantization` argument of `ClipSlot.fire(record_length=..., launch_quantization=...)`, so the LOM's untyped `launch_quantization (optional)` parameter of `ClipSlot.fire` takes a `Live.Song.Quantization` member in Python.

## `Live.Song.RecordingQuantization` (Live 12, Ableton's `pushbase/quantization_component.py` and `ableton/v3/.../clip_actions.py`)

Labels are the ones `clip_actions.py` shows next to each member.

| Member | Label |
|---|---|
| `rec_q_no_q` | None |
| `rec_q_quarter` | 1/4 |
| `rec_q_eight` | 1/8 |
| `rec_q_eight_triplet` | 1/8T |
| `rec_q_eight_eight_triplet` | 1/8+T |
| `rec_q_sixtenth` | 1/16 |
| `rec_q_sixtenth_triplet` | 1/16T |
| `rec_q_sixtenth_sixtenth_triplet` | 1/16+T |
| `rec_q_thirtysecond` | 1/32 |

`Song.midi_recording_quantization` is set with these members (`quantization_component.py`).

## `Live.Clip.LaunchMode` (third-party evidence only)

Not among the `ENUM_TYPES` of `LomTypes.py`; the LOM types `Clip.launch_mode` as int (0 = Trigger, 1 = Gate, 2 = Toggle, 3 = Repeat). ableton-js sets it with `getattr(Live.Clip.LaunchMode, value)` where `value` is one of `trigger`, `gate`, `toggle`, `repeat`. Not verified against an Ableton script; setting the int is what the LOM documents.

## `Live.Clip.ClipLaunchQuantization` (third-party evidence only)

The LOM types `Clip.launch_quantization` as int (0 = Global … 14 = 1/32). ableton-js sets it with `getattr(Live.Clip.ClipLaunchQuantization, value)` where `value` is one of `q_global`, `q_none`, `q_8_bars`, `q_4_bars`, `q_2_bars`, `q_bar`, `q_half`, `q_half_triplet`, `q_quarter`, `q_quarter_triplet`, `q_eighth`, `q_eighth_triplet`, `q_sixteenth`, `q_sixteenth_triplet`, `q_thirtysecond`. Those spellings differ from `Live.Song.Quantization` and are not verified against an Ableton script; prefer the int.
