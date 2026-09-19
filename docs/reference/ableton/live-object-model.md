# LOM - The Live Object Model

| | |
|---|---|
| Source | https://docs.cycling74.com/max8/vignettes/live_object_model (served from https://docs.cycling74.com/legacy/max8/vignettes/live_object_model) |
| Fetched | 2026-09-19 |
| Live version documented | Live 12.1 (page states "This document refers to Ableton Live version 12.1") |
| Copyright | Cycling '74 (the Live Object Model is documented by Cycling '74 for Max for Live; Live itself is Ableton AG's). Stored here as a local reference copy; not our text. |

*Objects which comprise the Live API described by their structure, properties and functions.*

## Introduction

The Live Object Model lists a number of Live object classes with their properties and functions, as well as their parent-child relations through which a hierarchy is formed. Please refer to the [Live API overview chapter](https://docs.cycling74.com/max8/vignettes/live_api_overview) for definitions of the basic Live API terms and a list of the Max objects used to access it.

*This document refers to Ableton Live version 12.1*

## Object Model Overview

The page shows a clickable class map (an SVG image, not reproduced here). The table below lists every class on the page with its canonical path; the classes marked **full** are reproduced in full below, the others only by their one-line description.

| Class | Canonical path | Kept |
|---|---|---|
| [Application](#application) | `live_app` | summary |
| [Application.View](#applicationview) | `live_app view` | summary |
| [TuningSystem](#tuningsystem) | `live_set tuning_system` | summary |
| [Song](#song) | `live_set` | full |
| [Song.View](#songview) | `live_set view` | full |
| [GroovePool](#groovepool) | `live_set groove_pool` | summary |
| [Track](#track) | `live_set tracks N` | full |
| [Track.View](#trackview) | `live_set tracks N view` | summary |
| [ClipSlot](#clipslot) | `live_set tracks N clip_slots M` | full |
| [Clip](#clip) | `live_set tracks N clip_slots M clip` | full |
| [Clip.View](#clipview) | `live_set tracks N clip_slots M clip view` | summary |
| [Groove](#groove) | `live_set groove_pool grooves N` | summary |
| [Device](#device) | `live_set tracks N devices M` | summary |
| [Device.View](#deviceview) | `live_set tracks N devices M view` | summary |
| [DeviceParameter](#deviceparameter) | `live_set tracks N devices M parameters L` | full |
| [RackDevice](#rackdevice) | `—` | summary |
| [RackDevice.View](#rackdeviceview) | `—` | summary |
| [DrumPad](#drumpad) | `live_set tracks N devices M drum_pads L` | summary |
| [Chain](#chain) | `live_set tracks N devices M chains L` | summary |
| [DrumChain](#drumchain) | `—` | summary |
| [ChainMixerDevice](#chainmixerdevice) | `live_set tracks N devices M chains L mixer_device` | summary |
| [ShifterDevice](#shifterdevice) | `—` | summary |
| [SimplerDevice](#simplerdevice) | `—` | summary |
| [SimplerDevice.View](#simplerdeviceview) | `—` | summary |
| [Sample](#sample) | `live_set tracks N devices N sample` | summary |
| [WavetableDevice](#wavetabledevice) | `—` | summary |
| [CompressorDevice](#compressordevice) | `—` | summary |
| [PluginDevice](#plugindevice) | `—` | summary |
| [MaxDevice](#maxdevice) | `—` | summary |
| [MixerDevice](#mixerdevice) | `live_set tracks N mixer_device` | full |
| [Eq8Device](#eq8device) | `—` | summary |
| [Eq8Device.View](#eq8deviceview) | `—` | summary |
| [DriftDevice](#driftdevice) | `—` | summary |
| [DrumCellDevice](#drumcelldevice) | `—` | summary |
| [HybridReverbDevice](#hybridreverbdevice) | `—` | summary |
| [MeldDevice](#melddevice) | `—` | summary |
| [RoarDevice](#roardevice) | `—` | summary |
| [SpectralResonatorDevice](#spectralresonatordevice) | `—` | summary |
| [LooperDevice](#looperdevice) | `—` | summary |
| [DeviceIO](#deviceio) | `—` | summary |
| [Scene](#scene) | `live_set scenes N` | full |
| [CuePoint](#cuepoint) | `live_set cue_points N` | full |
| [ControlSurface](#controlsurface) | `control_surfaces N` | summary |
| [this_device](#this_device) | `live_set tracks N devices M` | summary |

## Song

This class represents a Live Set. The current Live Set is reachable by the root path live_set.

**Canonical path:** `live_set`

### Children

#### `cue_points`

Type: list of CuePoint — Access: get, observe

Cue points are the markers in the Arrangement to which you can jump.

#### `return_tracks`

Type: list of Track — Access: get, observe

#### `scenes`

Type: list of Scene — Access: get, observe

#### `tracks`

Type: list of Track — Access: get, observe

#### `visible_tracks`

Type: list of Track — Access: get, observe

A track is visible if it's not part of a folded group. If a track is scrolled out of view it's still considered visible.

#### `master_track`

Type: Track — Access: get

#### `view`

Type: Song.View — Access: get

#### `groove_pool`

Type: GroovePool — Access: get

Live's groove pool.

*Available since Live 11.0.*

#### `tuning_system`

Type: TuningSystem — Access: get, observe

Live's currently active tuning system.

### Properties

#### `appointed_device`

Type: Device — Access: get, observe

The appointed device is the one used by a control surface unless the control surface itself chooses which device to use. It is marked by a blue hand.

#### `arrangement_overdub`

Type: bool — Access: get, set, observe

Get/set the state of the MIDI Arrangement Overdub button.

#### `back_to_arranger`

Type: bool — Access: get, set, observe

Get/set/observe the current state of the Back to Arrangement button located in Live's transport bar (1 = highlighted). This button is used to indicate that the current state of the playback differs from what is stored in the Arrangement.

Setting this property to 0 will make Live go back to playing the content of the arrangement.

#### `can_capture_midi`

Type: bool — Access: get, observe

1 = Recently played MIDI material exists that can be captured into a Live Track. See *capture_midi*.

#### `can_jump_to_next_cue`

Type: bool — Access: get, observe

0 = there is no cue point to the right of the current one, or none at all.

#### `can_jump_to_prev_cue`

Type: bool — Access: get, observe

0 = there is no cue point to the left of the current one, or none at all.

#### `can_redo`

Type: bool — Access: get

1 = there is something in the history to redo.

#### `can_undo`

Type: bool — Access: get

1 = there is something in the history to undo.

#### `clip_trigger_quantization`

Type: int — Access: get, set, observe

Reflects the quantization setting in the transport bar. \
0 = None \
1 = 8 Bars \
2 = 4 Bars \
3 = 2 Bars \
4 = 1 Bar \
5 = 1/2 \
6 = 1/2T \
7 = 1/4 \
8 = 1/4T \
9 = 1/8 \
10 = 1/8T \
11 = 1/16 \
12 = 1/16T \
13 = 1/32

#### `count_in_duration`

Type: int — Access: get, observe

The duration of the Metronome's Count-In setting as an index, mapped as follows: \
0 = None \
1 = 1 Bar \
2 = 2 Bars \
3 = 4 Bars

#### `current_song_time`

Type: float — Access: get, set, observe

The playing position in the Live Set, in beats.

#### `exclusive_arm`

Type: bool — Access: get

Current status of the exclusive Arm option set in the Live preferences.

#### `exclusive_solo`

Type: bool — Access: get

Current status of the exclusive Solo option set in the Live preferences.

#### `file_path`

Type: symbol — Access: get

The path to the current Live Set, in OS-native format. If the Live Set hasn't been saved, the path is empty.

#### `groove_amount`

Type: float — Access: get, set, observe

The groove amount from the current set's groove pool (0. - 1.0).

#### `is_ableton_link_enabled`

Type: bool — Access: get, set, observe

Enable/disable Ableton Link. The Link toggle in the Live's transport bar must be visible to enable Link.

#### `is_ableton_link_start_stop_sync_enabled`

Type: bool — Access: get, set, observe

Enable/disable Ableton Link Start Stop Sync.

#### `is_counting_in`

Type: bool — Access: get, observe

1 = the Metronome is currently counting in.

#### `is_playing`

Type: bool — Access: get, set, observe

Get/set if Live's transport is running.

#### `last_event_time`

Type: float — Access: get

The beat time of the last event (i.e. automation breakpoint, clip end, cue point, loop end) in the Arrangement.

#### `loop`

Type: bool — Access: get, set, observe

Get/set the enabled state of the Arrangement loop.

#### `loop_length`

Type: float — Access: get, set, observe

Arrangement loop length in beats.

#### `loop_start`

Type: float — Access: get, set, observe

Arrangement loop start in beats.

#### `metronome`

Type: bool — Access: get, set, observe

Get/set the enabled state of the metronome.

#### `midi_recording_quantization`

Type: int — Access: get, set, observe

Get/set the current Record Quantization value. \
0 = None \
1 = 1/4 \
2 = 1/8 \
3 = 1/8T \
4 = 1/8 + 1/8T \
5 = 1/16 \
6 = 1/16T \
7 = 1/16 + 1/16T \
8 = 1/32

#### `name`

Type: symbol — Access: get

The name of the current Live Set. If the Live Set hasn't been saved, the name is empty.

#### `nudge_down`

Type: bool — Access: get, set, observe

1 = the Tempo Nudge Down button in the transport bar is currently pressed.

#### `nudge_up`

Type: bool — Access: get, set, observe

1 = the Tempo Nudge Up button in the transport bar is currently pressed.

#### `tempo_follower_enabled`

Type: bool — Access: get, set, observe

1 = the Tempo Follower controls the tempo. The Tempo Follower Toggle must be made visible in the preferences for this property to be effective.

#### `overdub`

Type: bool — Access: get, set, observe

1 = MIDI Arrangement Overdub is enabled in the transport.

#### `punch_in`

Type: bool — Access: get, set, observe

1 = the Punch-In button is enabled in the transport.

#### `punch_out`

Type: bool — Access: get, set, observe

1 = the Punch-Out button is enabled in the transport.

#### `re_enable_automation_enabled`

Type: bool — Access: get, observe

1 = the Re-Enable Automation button is on.

#### `record_mode`

Type: bool — Access: get, set, observe

1 = the Arrangement Record button is on.

#### `root_note`

Type: int — Access: get, set, observe

The root note of the scale currently selected in Live. The root note can be a number between 0 and 11, where 0 = C and 11 = B.

#### `scale_intervals`

Type: list — Access: get, observe

A list of integers representing the intervals in Live's current scale (see *scale_name* and *scale_mode*). An interval is expressed as the difference between the scale degree at the list index and the first scale degree.

#### `scale_mode`

Type: bool — Access: get, set, observe

Access to the Scale Mode setting in Live.

When on, key tracks that belong to the currently selected scale are highlighted in Live's MIDI Note Editor, and pitch-based parameters in MIDI Tools and Devices can be edited in scale degrees rather than semitones.

See also *root_note*, *scale_name*, and *scale_intervals*.

#### `scale_name`

Type: unicode — Access: get, set, observe

The name of the scale selected in Live, as displayed in the Current Scale Name chooser.

#### `select_on_launch`

Type: bool — Access: get

1 = the "Select on Launch" option is set in Live's preferences.

#### `session_automation_record`

Type: bool — Access: get, set, observe

The state of the Automation Arm button.

#### `session_record`

Type: bool — Access: get, set, observe

The state of the Session Overdub button.

#### `session_record_status`

Type: int — Access: get, observe

Reflects the state of the Session Record button.

#### `signature_denominator`

Type: int — Access: get, set, observe

#### `signature_numerator`

Type: int — Access: get, set, observe

#### `song_length`

Type: float — Access: get, observe

A little more than last_event_time, in beats.

#### `start_time`

Type: float — Access: get, set, observe

The position in the Live Set where playing will start, in beats.

#### `swing_amount`

Type: float — Access: get, set, observe

Range: 0.0 - 1.0; affects MIDI Recording Quantization and all direct calls to Clip.quantize.

#### `tempo`

Type: float — Access: get, set, observe

Current tempo of the Live Set in BPM, 20.0... 999.0. The tempo may be automated, so it can change depending on the current song time.

### Functions

#### `capture_and_insert_scene`

Capture the currently playing clips and insert them as a new scene below the selected scene.

#### `capture_midi`

Parameter: *destination* [int] \
0 = auto, 1 = session, 2 = arrangement \
Capture recently played MIDI material from audible tracks into a Live Clip. \
If *destinaton* is not set or it is set to *auto*, the Clip is inserted into the view currently visible in the focused Live window. Otherwise, it is inserted into the specified view.

#### `continue_playing`

From the current playback position.

#### `create_audio_track`

Parameter: *index*\
Index determines where the track is added, it is only valid between 0 and len(song.tracks). Using an index of -1 will add the new track at the end of the list.

#### `create_midi_track`

Parameter: *index*\
Index determines where the track is added, it is only valid between 0 and len(song.tracks). Using an index of -1 will add the new track at the end of the list.

#### `create_return_track`

Adds a new return track at the end.

#### `create_scene`

Parameter: *index*\
Returns: The new scene \
Index determines where the scene is added. It is only valid between 0 and len(song.scenes). Using an index of -1 will add the new scene at the end of the list.

#### `delete_scene`

Parameter: *index*\
Delete the scene at the given index.

#### `delete_track`

Parameter: *index*\
Delete the track at the given index.

#### `delete_return_track`

Parameter: *index*\
Delete the return track at the given index.

#### `duplicate_scene`

Parameter: *index*\
Index determines which scene to duplicate.

#### `duplicate_track`

Parameter: *index*\
Index determines which track to duplicate.

#### `find_device_position`

Parameter: \
*device* [live object] \
*target* [live object] \
*target position* [int] \
Returns: \
[int] The position in the target's chain where the device can be inserted that is the closest possible to the target position.

#### `force_link_beat_time`

Force the Link timeline to jump to Live's current beat time.

#### `get_beats_loop_length`

Returns: *bars.beats.sixteenths.ticks* [symbol] \
The Arrangement loop length.

#### `get_beats_loop_start`

Returns: *bars.beats.sixteenths.ticks* [symbol] \
The Arrangement loop start.

#### `get_current_beats_song_time`

Returns: *bars.beats.sixteenths.ticks* [symbol] \
The current Arrangement playback position.

#### `get_current_smpte_song_time`

Parameter: *format*\
*format* [int] is the time code type to be returned \
0 = the frame position shows the milliseconds \
1 = Smpte24 \
2 = Smpte25 \
3 = Smpte30 \
4 = Smpte30Drop \
5 = Smpte29 \
Returns: *hours:min:sec:frames* [symbol] \
The current Arrangement playback position.

#### `is_cue_point_selected`

Returns: bool 1 = the current Arrangement playback position is at a cue point

#### `jump_by`

Parameter: *beats*\
*beats* [double] is the amount to jump relatively to the current position

#### `jump_to_next_cue`

Jump to the right, if possible.

#### `jump_to_prev_cue`

Jump to the left, if possible.

#### `move_device`

Parameter: \
*device* [live object] \
*target* [live object] \
*target position* [int] \
Returns: [int] The position in the target's chain where the device was inserted. \
Move the device to the specified position in the target chain. If the device cannot be moved to the specified position, the nearest possible position is chosen.

#### `play_selection`

Do nothing if no selection is set in Arrangement, or play the current selection.

#### `re_enable_automation`

Trigger 'Re-Enable Automation', re-activating automation in all running Session clips.

#### `redo`

Causes the Live application to redo the last operation.

#### `scrub_by`

Parameter: *beats*\
*beats* [double] the amount to scrub relative to the current Arrangement playback position \
Same as jump_by, at the moment.

#### `set_or_delete_cue`

Toggle cue point at current Arrangement playback position.

#### `start_playing`

Start playback from the insert marker.

#### `stop_all_clips`

Parameter (optional): *quantized*\
Calling the function with 0 will stop all clips immediately, independent of the launch quantization. The default is '1'.

#### `stop_playing`

Stop the playback.

#### `tap_tempo`

Same as pressing the Tap Tempo button in the transport bar. The new tempo is calculated based on the time between subsequent calls of this function.

#### `trigger_session_record`

Parameter: *record_length (optional)*\
Starts recording in either the selected slot or the next empty slot, if the track is armed. If *record_length* is provided, the slot will record for the given length in beats. \
If triggered while recording, recording will stop and clip playback will start.

#### `undo`

Causes the Live application to undo the last operation.

## Song.View

This class represents the view aspects of a Live document: the Session and Arrangement Views.

**Canonical path:** `live_set view`

### Children

#### `detail_clip`

Type: Clip — Access: get, set, observe

The clip currently displayed in the Live application's Detail View.

#### `highlighted_clip_slot`

Type: ClipSlot — Access: get, set

The slot highlighted in the Session View.

#### `selected_chain`

Type: Chain — Access: get, set, observe

The highlighted chain, or "id 0"

#### `selected_parameter`

Type: DeviceParameter — Access: get, observe

The selected parameter, or "id 0"

#### `selected_scene`

Type: Scene — Access: get, set, observe

#### `selected_track`

Type: Track — Access: get, set, observe

### Properties

#### `draw_mode`

Type: bool — Access: get, set, observe

Reflects the state of the envelope/automation Draw Mode Switch in the transport bar, as toggled with Cmd/Ctrl-B. \
0 = breakpoint editing (shows arrow), 1 = drawing (shows pencil)

#### `follow_song`

Type: bool — Access: get, set, observe

Reflects the state of the Follow switch in the transport bar as toggled with Cmd/Ctrl-F. \
0 = don't follow playback position, 1 = follow playback position

### Functions

#### `select_device`

Parameter: *id NN*\
Selects the given device object in its track. \
You may obtain the id using a [live.path](https://docs.cycling74.com/max8/refpages/live.path) or by using get devices on a track, for example. \
The track containing the device will not be shown automatically, and the device gets the appointed device (blue hand) only if its track is selected.

## Track

This class represents a track in Live. It can either be an audio track, a MIDI track, a return track or the master track. The master track and at least one Audio or MIDI track will be always present. Return tracks are optional.

Not all properties are supported by all types of tracks. The properties are marked accordingly.

**Canonical path:** `live_set tracks N`

### Children

#### `clip_slots`

Type: list of ClipSlot — Access: get, observe

#### `arrangement_clips`

Type: list of Clip — Access: get, observe

The list of this track's Arrangement View clip IDs

*Available since Live 11.0.*

#### `devices`

Type: list of Device — Access: get, observe

Includes mixer device.

#### `group_track`

Type: Track — Access: get

The Group Track, if the Track is grouped. If it is not, *id 0* is returned.

#### `mixer_device`

Type: MixerDevice — Access: get

#### `view`

Type: Track.View — Access: get

### Properties

#### `arm`

Type: bool — Access: get, set, observe

1 = track is armed for recording. [not in return/master tracks]

#### `available_input_routing_channels`

Type: dictionary — Access: get, observe

The list of available source channels for the track's input routing. It's represented as a *dictionary* with the following key:\
*available_input_routing_channels* [list] \
The list contains *dictionaries* as described in *input_routing_channel*. \
Only available on MIDI and audio tracks.

#### `available_input_routing_types`

Type: dictionary — Access: get, observe

The list of available source types for the track's input routing. It's represented as a *dictionary* with the following key:\
*available_input_routing_types* [list] \
The list contains *dictionaries* as described in *input_routing_type*. \
Only available on MIDI and audio tracks.

#### `available_output_routing_channels`

Type: dictionary — Access: get, observe

The list of available target channels for the track's output routing. It's represented as a *dictionary* with the following key:\
*available_output_routing_channels* [list] \
The list contains *dictionaries* as described in *output_routing_channel*. \
Not available on the master track.

#### `available_output_routing_types`

Type: dictionary — Access: get, observe

The list of available target types for the track's output routing. It's represented as a *dictionary* with the following key:\
*available_output_routing_types* [list] \
The list contains *dictionaries* as described in *output_routing_type*. \
Not available on the master track.

#### `back_to_arranger`

Type: bool — Access: get, set, observe

Get/set/observe the current state of the Single Track Back to Arrangement button (1 = highlighted). This button is used to indicate that the current state of the playback differs from what is stored in the Arrangement.

Setting this property to 0 will make Live go back to playing the track's arrangement content. For group tracks, this means that all of the tracks that belong to the group and any subgroups will go back to playing the arrangement.

#### `can_be_armed`

Type: bool — Access: get

0 for return and master tracks.

#### `can_be_frozen`

Type: bool — Access: get

1 = the track can be frozen, 0 = otherwise.

#### `can_show_chains`

Type: bool — Access: get

1 = the track contains an Instrument Rack device that can show chains in Session View.

#### `color`

Type: int — Access: get, set, observe

The RGB value of the track's color in the form 0x00rrggbb or (2^16 * red) + (2^8) * green + blue, where red, green and blue are values from 0 (dark) to 255 (light).

When setting the RGB value, the nearest color from the track color chooser is taken.

#### `color_index`

Type: long — Access: get, set, observe

The color index of the track.

#### `fired_slot_index`

Type: int — Access: get, observe

Reflects the blinking clip slot.
-1 = no slot fired, -2 = Clip Stop Button fired \
First clip slot has index 0. \
[not in return/master tracks]

#### `fold_state`

Type: int — Access: get, set

0 = tracks within the Group Track are visible, 1 = Group Track is folded and the tracks within the Group Track are hidden \
[only available if is_foldable = 1]

#### `has_audio_input`

Type: bool — Access: get

1 for audio tracks.

#### `has_audio_output`

Type: bool — Access: get

1 for audio tracks and MIDI tracks with instruments.

#### `has_midi_input`

Type: bool — Access: get

1 for MIDI tracks.

#### `has_midi_output`

Type: bool — Access: get

1 for MIDI tracks with no instruments and no audio effects.

#### `implicit_arm`

Type: bool — Access: get, set, observe

A second arm state, only used by Push so far.

#### `input_meter_left`

Type: float — Access: get, observe

Smoothed momentary peak value of left channel input meter, 0.0 to 1.0. For tracks with audio output only. This value corresponds to the meters shown in Live. Please take into account that the left/right audio meters put a significant load onto the GUI part of Live.

#### `input_meter_level`

Type: float — Access: get, observe

Hold peak value of input meters of audio and MIDI tracks, 0.0... 1.0. For audio tracks it is the maximum of the left and right channels. The hold time is 1 second.

#### `input_meter_right`

Type: float — Access: get, observe

Smoothed momentary peak value of right channel input meter, 0.0 to 1.0. For tracks with audio output only. This value corresponds to the meters shown in Live.

#### `input_routing_channel`

Type: dictionary — Access: get, set, observe

The currently selected source channel for the track's input routing. It's represented as a *dictionary* with the following keys:\
*display_name* [symbol] \
*identifier* [symbol] \
Can be set to all values found in the track's *available_input_routing_channels*. \
Only available on MIDI and audio tracks.

#### `input_routing_type`

Type: dictionary — Access: get, set, observe

The currently selected source type for the track's input routing. It's represented as a *dictionary* with the following keys:\
*display_name* [symbol] \
*identifier* [symbol] \
Can be set to all values found in the track's *available_input_routing_types*. \
Only available on MIDI and audio tracks.

#### `is_foldable`

Type: bool — Access: get

1 = track can be (un)folded to hide or reveal the contained tracks. This is currently the case for Group Tracks. Instrument and Drum Racks return 0 although they can be opened/closed. This will be fixed in a later release.

#### `is_frozen`

Type: bool — Access: get, observe

1 = the track is currently frozen.

#### `is_grouped`

Type: bool — Access: get

1 = the track is contained within a Group Track.

#### `is_part_of_selection`

Type: bool — Access: get

#### `is_showing_chains`

Type: bool — Access: get, set, observe

Get or set whether a track with an Instrument Rack device is currently showing its chains in Session View.

#### `is_visible`

Type: bool — Access: get

0 = track is hidden in a folded Group Track.

#### `mute`

Type: bool — Access: get, set, observe

[not in master track]

#### `muted_via_solo`

Type: bool — Access: get, observe

1 = the track or chain is muted due to Solo being active on at least one other track.

#### `name`

Type: symbol — Access: get, set, observe

As shown in track header.

#### `output_meter_left`

Type: float — Access: get, observe

Smoothed momentary peak value of left channel output meter, 0.0 to 1.0. For tracks with audio output only. This value corresponds to the meters shown in Live. Please take into account that the left/right audio meters add a significant load to Live GUI resource usage.

#### `output_meter_level`

Type: float — Access: get, observe

Hold peak value of output meters of audio and MIDI tracks, 0.0 to 1.0. For audio tracks, it is the maximum of the left and right channels. The hold time is 1 second.

#### `output_meter_right`

Type: float — Access: get, observe

Smoothed momentary peak value of right channel output meter, 0.0 to 1.0. For tracks with audio output only. This value corresponds to the meters shown in Live.

#### `performance_impact`

Type: float — Access: get, observe

Reports the performance impact of this track.

#### `output_routing_channel`

Type: dictionary — Access: get, set, observe

The currently selected target channel for the track's output routing. It's represented as a *dictionary* with the following keys:\
*display_name* [symbol] \
*identifier* [symbol] \
Can be set to all values found in the track's *available_output_routing_channels*. \
Not available on the master track.

#### `output_routing_type`

Type: dictionary — Access: get, set, observe

The currently selected target type for the track's output routing. It's represented as a *dictionary* with the following keys:\
*display_name* [symbol] \
*identifier* [symbol] \
Can be set to all values found in the track's *available_output_routing_types*. \
Not available on the master track.

#### `playing_slot_index`

Type: int — Access: get, observe

First slot has index 0, -2 = Clip Stop slot fired in Session View, -1 = Arrangement recording with no Session clip playing. [not in return/master tracks]

#### `solo`

Type: bool — Access: get, set, observe

Remark: when setting this property, the exclusive Solo logic is bypassed, so you have to unsolo the other tracks yourself. [not in master track]

### Functions

#### `create_audio_clip`

Parameters: \
*file_path* [symbol] \
*position* [float] \
Given an absolute path to a valid audio file in a supported format, creates an audio clip that references the file at the specified position in the arrangement view. Prints an error if the track is not an audio track, if the track is frozen, or if the track is being recorded into. The position must be within the range [0., 1576800].

See the *ClipSlot.create_audio_clip* function if you need to create audio clips in session view instead.

#### `delete_clip`

Parameter: *clip*\
Delete the given clip.

#### `delete_device`

Parameter: *index*\
Delete the device at the given index.

#### `duplicate_clip_slot`

Parameter: *index*\
Works like 'Duplicate' in a clip's context menu.

#### `duplicate_clip_to_arrangement`

Parameters: *clip*\
*destination_time* [double] \
Duplicate the given clip to the Arrangement, placing it at the given *destination_time* in beats.

#### `jump_in_running_session_clip`

Parameter: *beats*\
*beats* [double] is the amount to jump relatively to the current clip position. \
Modify playback position in running Session clip, if any.

#### `stop_all_clips`

Stops all playing and fired clips in this track.

## ClipSlot

This class represents an entry in Live's Session View matrix.

The properties playing_status, is_playing and is_recording are useful for clip slots of Group Tracks. These are always empty and represent the state of the clips in the tracks within the Group Track.

**Canonical path:** `live_set tracks N clip_slots M`

### Children

#### `clip`

Type: Clip — Access: get

id 0 if slot is empty

### Properties

#### `color`

Type: long — Access: get, observe

The color of the first clip in the Group Track if the clip slot is a Group Track slot.

#### `color_index`

Type: long — Access: get, observe

The color index of the first clip in the Group Track if the clip slot is a Group Track slot.

#### `controls_other_clips`

Type: bool — Access: get, observe

1 for a Group Track slot that has non-deactivated clips in the tracks within its group. \
Control of empty clip slots doesn't count.

#### `has_clip`

Type: bool — Access: get, observe

1 = a clip exists in this clip slot.

#### `has_stop_button`

Type: bool — Access: get, set, observe

1 = this clip stops its track (or tracks within a Group Track).

#### `is_group_slot`

Type: bool — Access: get

1 = this clip slot is a Group Track slot.

#### `is_playing`

Type: bool — Access: get

1 = playing_status != 0, otherwise 0.

#### `is_recording`

Type: bool — Access: get

1 = playing_status == 2, otherwise 0.

#### `is_triggered`

Type: bool — Access: get, observe

1 = clip slot button (Clip Launch, Clip Stop or Clip Record) or button of contained clip are blinking.

#### `playing_status`

Type: int — Access: get, observe

0 = all clips in tracks within a Group Track stopped or all tracks within a Group Track are empty. \
1 = at least one clip in a track within a Group Track is playing. \
2 = at least one clip in a track within a Group Track is playing or recording. \
Equals 0 if this is not a clip slot of a Group Track.

#### `will_record_on_start`

Type: bool — Access: get

1 = clip slot will record on start.

### Functions

#### `create_audio_clip`

Parameter: *path*\
Given an absolute path to a valid audio file in a supported format, creates an audio clip that references the file in the clip slot. Throws an error if the clip slot doesn't belong to an audio track or if the track is frozen.

#### `create_clip`

Parameter: *length*\
Length is given in beats and must be a greater value than 0.0. Can only be called on empty clip slots in MIDI tracks.

#### `delete_clip`

Deletes the contained clip.

#### `duplicate_clip_to`

Parameter: *target_clip_slot* [ClipSlot] \
Duplicates the slot's clip to the given clip slot, overriding the target clip slot's clip if it's not empty.

#### `fire`

Parameter: *record_length (optional)*\
*launch_quantization (optional)*\
Fires the clip or triggers the Stop Button, if any. Starts recording if slot is empty and track is armed. Starts recording of armed and empty tracks within a Group Track if Preferences->Launch->Start Recording on Scene Launch is ON. If *record_length* is provided, the slot will record for the given length in beats. *launch_quantization* overrides the global quantization if provided.

#### `set_fire_button_state`

Parameter: *state* [bool] \
1 = Live simulates pressing of Clip Launch button until the state is set to 0 or until the slot is stopped otherwise.

#### `stop`

Stops playing or recording clips in this track or the tracks within the group, if any. It doesn't matter on which slot of the track you call this function.

## Clip

This class represents a clip in Live. It can be either an audio clip or a MIDI clip in the Arrangement or Session View, depending on the track / slot it lives in.

**Canonical path:** `live_set tracks N clip_slots M clip`

### Children

#### `view`

Type: Clip.View — Access: get

### Properties

#### `available_warp_modes`

Type: list — Access: get

Returns the list of indexes of the Warp Modes available for the clip. Only valid for audio clips.

#### `color`

Type: int — Access: get, set, observe

The RGB value of the clip's color in the form 0x00rrggbb or (2^16 * red) + (2^8) * green + blue, where red, green and blue are values from 0 (dark) to 255 (light).

When setting the RGB value, the nearest color from the clip color chooser is taken.

#### `color_index`

Type: int — Access: get, set, observe

The clip's color index.

#### `end_marker`

Type: double — Access: get, set, observe

The end marker of the clip in beats, independent of the loop state. Cannot be set before the start marker.

#### `end_time`

Type: double — Access: get, observe

The end time of the clip. For Session View clips, if Loop is on, this is the Loop End, otherwise it's the End Marker. For Arrangement View clips, this is always the position of the clip's rightmost edge in the Arrangement.

#### `gain`

Type: double — Access: get, set, observe

The gain of the clip (range is 0.0 to 1.0). Only valid for audio clips.

#### `gain_display_string`

Type: symbol — Access: get

Get the gain display value of the clip as a string (e.g. "1.3 dB"). Can only be called on audio clips.

#### `file_path`

Type: symbol — Access: get

Get the location of the audio file represented by the clip. Only available for audio clips.

#### `groove`

Type: Groove — Access: get, set, observe

Get/set/observe access to the groove associated with this clip.

*Available since Live 11.0.*

#### `has_envelopes`

Type: bool — Access: get, observe

Get/observe whether the clip has any automation.

#### `has_groove`

Type: bool — Access: get

Returns true if a groove is associated with this clip.

*Available since Live 11.0.*

#### `is_arrangement_clip`

Type: bool — Access: get

1 = The clip is an Arrangement clip. \
A clip can be either an Arrangement or a Session clip.

#### `is_audio_clip`

Type: bool — Access: get

0 = MIDI clip, 1 = audio clip

#### `is_midi_clip`

Type: bool — Access: get

The opposite of is_audio_clip.

#### `is_overdubbing`

Type: bool — Access: get, observe

1 = clip is overdubbing.

#### `is_playing`

Type: bool — Access: get, set

1 = clip is playing or recording.

#### `is_recording`

Type: bool — Access: get, observe

1 = clip is recording.

#### `is_triggered`

Type: bool — Access: get

1 = Clip Launch button is blinking.

#### `launch_mode`

Type: int — Access: get, set, observe

The Launch Mode of the Clip as an integer index. Available Launch Modes are: \
0 = Trigger (default) \
1 = Gate \
2 = Toggle \
3 = Repeat

*Available since Live 11.0.*

#### `launch_quantization`

Type: int — Access: get, set, observe

The Launch Quantization of the Clip as an integer index. Available Launch Quantization values are: \
0 = Global (default) \
1 = None \
2 = 8 Bars \
3 = 4 Bars \
4 = 2 Bars \
5 = 1 Bar \
6 = 1/2 \
7 = 1/2T \
8 = 1/4 \
9 = 1/4T \
10 = 1/8 \
11 = 1/8T \
12 = 1/16 \
13 = 1/16T \
14 = 1/32

*Available since Live 11.0.*

#### `legato`

Type: bool — Access: get, set, observe

1 = Legato Mode switch in the Clip's Launch settings is on.

*Available since Live 11.0.*

#### `length`

Type: double — Access: get

For looped clips: loop length in beats. Otherwise it's the distance in beats from start to end marker. Makes no sense for unwarped audio clips.

#### `loop_end`

Type: double — Access: get, set, observe

For looped clips: loop end. \
For unlooped clips: clip end.

#### `loop_jump`

Type: bang — Access: observe

Bangs when the clip play position is crossing the loop start marker (possibly projected into the loop).

#### `loop_start`

Type: double — Access: get, set, observe

For looped clips: loop start. \
For unlooped clips: clip start.

loop_start and loop_end are in absolute clip beat time if clip is MIDI or warped. The 1.1.1 position has beat time 0. If the clip is unwarped audio, they are given in seconds, 0 is the time of the first sample in the audio material.

#### `looping`

Type: bool — Access: get, set, observe

1 = clip is looped. Unwarped audio cannot be looped.

#### `muted`

Type: bool — Access: get, set, observe

1 = muted (i.e. the Clip Activator button of the clip is off).

#### `name`

Type: symbol — Access: get, set, observe

#### `notes`

Type: bang — Access: observe

Observer sends bang when the list of notes changes. \
Available for MIDI clips only.

#### `warp_markers`

Type: dict/bang — Access: get, observe

The Clip's Warp Markers as a dict. Observing this property bangs when the warp_markers change.

The last Warp Marker in the dict is not visible in the Live interface. This hidden marker is used to calculate the BPM of the last segment.

Available for audio clips only.

*Getting is available since Live 11.0.*

#### `pitch_coarse`

Type: int — Access: get, set, observe

Pitch shift in semitones ("Transpose"), -48... 48. \
Available for audio clips only.

#### `pitch_fine`

Type: float — Access: get, set, observe

Extra pitch shift in cents ("Detune"), -50... 49. \
Available for audio clips only.

#### `playing_position`

Type: float — Access: get, observe

Current playing position of the clip.

For MIDI and warped audio clips, the value is given in beats of absolute clip time. The clip's beat time of 0 is where 1 is shown in the bar/beat/16th time scale at the top of the clip view.

For unwarped audio clips, the position is given in seconds, according to the time scale shown at the bottom of the clip view.

Stopped clips have a playing position of 0.

#### `playing_status`

Type: bang — Access: observe

Observer sends bang when playing/trigger status changes.

#### `position`

Type: float — Access: get, observe

Get and set the clip's loop position. The value will always equal loop_start, however setting this property, unlike setting loop_start, preserves the loop length.

#### `ram_mode`

Type: bool — Access: get, set, observe

1 = an audio clip’s RAM switch is enabled.

#### `sample_length`

Type: int — Access: get

Length of the Clip's sample, in samples.

#### `sample_rate`

Type: float — Access: get

Get the Clip's sample rate.

#### `signature_denominator`

Type: int — Access: get, set, observe

#### `signature_numerator`

Type: int — Access: get, set, observe

#### `start_marker`

Type: double — Access: get, set, observe

The start marker of the clip in beats, independent of the loop state. Cannot be set behind the end marker.

#### `start_time`

Type: double — Access: get

The start time of the clip, relative to the global song time. For Session View clips, this is the time the clip was started. For Arrangement View clips, this is the offset within the arrangement. The value is in beats.

#### `velocity_amount`

Type: float — Access: get, set, observe

How much the velocity of the note that triggers the clip affects its volume, 0 = no effect, 1 = full effect.

*Available since Live 11.0.*

#### `warp_mode`

Type: int — Access: get, set, observe

The Warp Mode of the clip as an integer index. Available Warp Modes are: \
0 = Beats Mode \
1 = Tones Mode \
2 = Texture Mode \
3 = Re-Pitch Mode \
4 = Complex Mode \
5 = REX Mode \
6 = Complex Pro Mode \
Available for audio clips only.

#### `warping`

Type: bool — Access: get, set, observe

1 = Warp switch is on. \
Available for audio clips only.

#### `will_record_on_start`

Type: bool — Access: get

1 for MIDI clips which are in triggered state, with the track armed and MIDI Arrangement Overdub on.

### Functions

#### `add_new_notes`

Parameter: \
*dictionary*\
Key: *"notes"* [list of note specification dictionaries] \
Note specification dictionaries have the following keys: \
*pitch*: [int] the MIDI note number, 0...127, 60 is C3. \
*start_time*: [float] the note start time in beats of absolute clip time. \
*duration*: [float] the note length in beats. \
*velocity (optional)*: [float] the note velocity, 0... 127 *(100 by default)*. \
*mute (optional)*: [bool] 1 = the note is deactivated *(0 by default)*. \
*probability (optional)*: [float] the chance that the note will be played: \
1.0 = the note is always played \
0.0 = the note is never played \
*(1.0 by default)*. \
*velocity_deviation (optional)*: [float] the range of velocity values at which the note can be played: \
0.0 = no deviation; the note will always play at the velocity specified by the *velocity* property
-127.0 to 127.0 = the note will be assigned a velocity value between *velocity* and *velocity + velocity_deviation*, inclusive; if the resulting range exceeds the limits of MIDI velocity (0 to 127), then it will be clamped within those limits \
*(0.0 by default)*. \
*release_velocity (optional)*: [float] the note release velocity *(64 by default)*. \
Returns a list of note IDs of the added notes.

For MIDI clips only.

*Available since Live 11.0.*

#### `add_warp_marker`

Only available for warped Audio Clips. Adds the specified warp marker, if possible.

The warp marker is specified as a dict which can have a *beat_time* and a *sample_time* key, both associated with float values. \
The *sample_time* key may be omitted; in this case, Live will calculate the appropriate sample time to create a warp marker at the specified beat time without changing the Clip's playback timing, similar to what would happen if you were to double-click in the upper half of the Sample Display in Clip View.

If *sample_time* is specified, certain limitations must be taken into account:

-  The sample time must lie within the range *[0, s]*, where *s* is the sample's length. The *sample_length* Clip property helps with this.
-  The sample time must lie between the left and right adjacents markers' respective sample times (this is a logical constraint).
-  Within these constraints, there are limitations on the resulting segments' BPM. The allowed BPM range is *[5, 999]*.

#### `apply_note_modifications`

Parameter: \
*dictionary*\
Key: *"notes"* [list of note dictionaries] as returned from get_notes_extended. \
The list of note dictionaries passed to the function can be a subset of notes in the clip, but will be ignored if it contains any notes that are not present in the clip.

For MIDI clips only.

*Available since Live 11.0. Replaces modifying notes with remove_notes followed by set_notes.*

#### `clear_all_envelopes`

Removes all automation in the clip.

#### `clear_envelope`

Parameter: \
*device_parameter* [id] \
Removes the automation of the clip for the given parameter.

#### `crop`

Crops the clip: if the clip is looped, the region outside the loop is removed; if it isn't, the region outside the start and end markers.

#### `deselect_all_notes`

Call this before replace_selected_notes if you just want to add some notes. \
Output: \
deselect_all_notes id 0

For MIDI clips only.

#### `duplicate_loop`

Makes the loop two times longer by moving loop_end to the right, and duplicates both the notes and the envelopes. If the clip is not looped, the clip start/end range is duplicated. Available for MIDI clips only.

#### `duplicate_notes_by_id`

Parameter: \
*list* of note IDs. \
Or *dictionary*\
Keys: \
*note_ids* [list of note IDs] as returned from get_notes_extended \
*destination_time (optional)* [double/int] \
*transposition_amount (optional)* [int] \
Duplicates all notes matching the given note IDs. \
Provided note IDs must be associated with existing notes in the clip. Existing notes can be queried with get_notes_extended. \
The selection of notes will be duplicated to *destination_time*, if provided. Otherwise the new notes will be inserted after the last selected note. This behavior can be observed when duplicating notes in the Live GUI. \
If the *transposition_amount* is specified, the duplicated notes will be transposed by the number of semitones. \
Available for MIDI clips only.

*Available since Live 11.1.2*

#### `duplicate_region`

Parameter: \
*region_start* [double/int] \
*region_length* [double/int] \
*destination_time* [double/int] \
*pitch (optional)* [int] \
*transposition_amount (optional)* [int] \
Duplicate the notes in the specified region to the *destination_time*. Only notes of the specified pitch are duplicated or all if *pitch* is -1. If the *transposition_amount* is not 0, the notes in the region will be transposed by the *transpose_amount* of semitones. Available for MIDI clips only.

#### `fire`

Same effect as pressing the Clip Launch button.

#### `get_all_notes_extended`

Parameter: \
*dict (optional)* [dict] \
(See below for a discussion of this argument).

Returns a dictionary of all of the notes in the clip, regardless of where they are positioned with respect to the start/end markers and the loop start/loop end, as a list of note dictionaries. Each note dictionary consists of the following key-value pairs: \
*note_id*: [int] the unique note identifier. \
*pitch*: [int] the MIDI note number, 0...127, 60 is C3. \
*start_time*: [float] the note start time in beats of absolute clip time. \
*duration*: [float] the note length in beats. \
*velocity*: [float] the note velocity, 0... 127. \
*mute*: [bool] 1 = the note is deactivated. \
*probability*: [float] the chance that the note will be played: \
1.0 = the note is always played; \
0.0 = the note is never played. \
*velocity_deviation*: [float] the range of velocity values at which the note can be played: \
0.0 = no deviation; the note will always play at the velocity specified by the *velocity* property
-127.0 to 127.0 = the note will be assigned a velocity value between *velocity* and *velocity + velocity_deviation*, inclusive; if the resulting range exceeds the limits of MIDI velocity (0 to 127), then it will be clamped within those limits. \
*release_velocity*: [float] the note release velocity.

It is possible to optionally provide a single [dict] argument to this function, containing a single key-value pair: the key is "return" and the associated value is a list of the note properties as listed above in the discussion of the returned note dictionaries, e.g. ["note_id", "pitch", "velocity"]. The effect of this will be that the returned note dictionaries will only contain the key-value pairs for the specified properties, which can be useful to improve patch performance when processing large notes dictionaries.

For MIDI clips only.

*Available since Live 11.1*

#### `get_notes_by_id`

Parameter: \
*list* of note IDs.

Provided note IDs must be associated with existing notes in the clip. Existing notes can be queried with get_notes_extended.

Returns a dictionary of notes associated with the provided IDs, as a list of note dictionaries. Each note dictionary consists of the following key-value pairs: \
*note_id*: [int] the unique note identifier. \
*pitch*: [int] the MIDI note number, 0...127, 60 is C3. \
*start_time*: [float] the note start time in beats of absolute clip time. \
*duration*: [float] the note length in beats. \
*velocity*: [float] the note velocity, 0... 127. \
*mute*: [bool] 1 = the note is deactivated. \
*probability*: [float] the chance that the note will be played: \
1.0 = the note is always played; \
0.0 = the note is never played. \
*velocity_deviation*: [float] the range of velocity values at which the note can be played: \
0.0 = no deviation; the note will always play at the velocity specified by the *velocity* property
-127.0 to 127.0 = the note will be assigned a velocity value between *velocity* and *velocity + velocity_deviation*, inclusive; if the resulting range exceeds the limits of MIDI velocity (0 to 127), then it will be clamped within those limits. \
*release_velocity*: [float] the note release velocity.

It is possible to optionally provide the argument to this function in the form of a dictionary instead. The dictionary must include the "note_ids" key associated with a list of [int]s, which are the ID values you would like to pass to the function.

If you use this method, you can optionally provide an additional key-value pair: the key is "return" and the associated value is a list of the note properties as listed above in the discussion of the returned note dictionaries, e.g. ["note_id", "pitch", "velocity"]. The effect of this will be that the returned note dictionaries will only contain the key-value pairs for the specified properties, which can be useful to improve patch performance when processing large notes dictionaries.

For MIDI clips only.

*Available since Live 11.0.*

#### `get_notes_extended`

Parameters: \
*from_pitch* [int] \
*pitch_span* [int] \
*from_time* [float] \
*time_span* [float]

*from_time* and *time_span* are given in beats.

Returns a dictionary of notes that have their start times in the given area, as a list of note dictionaries. Each note dictionary consists of the following key-value pairs: \
*note_id*: [int] the unique note identifier. \
*pitch*: [int] the MIDI note number, 0...127, 60 is C3. \
*start_time*: [float] the note start time in beats of absolute clip time. \
*duration*: [float] the note length in beats. \
*velocity*: [float] the note velocity, 0... 127. \
*mute*: [bool] 1 = the note is deactivated. \
*probability*: [float] the chance that the note will be played: \
1.0 = the note is always played; \
0.0 = the note is never played. \
*velocity_deviation*: [float] the range of velocity values at which the note can be played: \
0.0 = no deviation; the note will always play at the velocity specified by the *velocity* property
-127.0 to 127.0 = the note will be assigned a velocity value between *velocity* and *velocity + velocity_deviation*, inclusive; if the resulting range exceeds the limits of MIDI velocity (0 to 127), then it will be clamped within those limits. \
*release_velocity*: [float] the note release velocity.

It is possible to optionally provide the arguments to this function in the form of a single dictionary instead. The dictionary must include all of the parameter names given above as its keys; the associated values are the parameter values you wish to pass to the function.

If you use this method, you can optionally provide an additional key-value pair: the key is "return" and the associated value is a list of the note properties as listed above in the discussion of the returned note dictionaries, e.g. ["note_id", "pitch", "velocity"]. The effect of this will be that the returned note dictionaries will only contain the key-value pairs for the specified properties, which can be useful to improve patch performance when processing large notes dictionaries.

For MIDI clips only.

*Available since Live 11.0. Replaces get_notes.*

#### `get_selected_notes_extended`

Parameter: \
*dict (optional)* [dict] \
(See below for a discussion of this argument).

Returns a dictionary of the selected notes in the clip, as a list of note dictionaries. Each note dictionary consists of the following key-value pairs: \
*note_id*: [int] the unique note identifier. \
*pitch*: [int] the MIDI note number, 0...127, 60 is C3. \
*start_time*: [float] the note start time in beats of absolute clip time. \
*duration*: [float] the note length in beats. \
*velocity*: [float] the note velocity, 0... 127. \
*mute*: [bool] 1 = the note is deactivated. \
*probability*: [float] the chance that the note will be played: \
1.0 = the note is always played; \
0.0 = the note is never played. \
*velocity_deviation*: [float] the range of velocity values at which the note can be played: \
0.0 = no deviation; the note will always play at the velocity specified by the *velocity* property
-127.0 to 127.0 = the note will be assigned a velocity value between *velocity* and *velocity + velocity_deviation*, inclusive; if the resulting range exceeds the limits of MIDI velocity (0 to 127), then it will be clamped within those limits. \
*release_velocity*: [float] the note release velocity.

It is possible to optionally provide a single [dict] argument to this function, containing a single key-value pair: the key is "return" and the associated value is a list of the note properties as listed above in the discussion of the returned note dictionaries, e.g. ["note_id", "pitch", "velocity"]. The effect of this will be that the returned note dictionaries will only contain the key-value pairs for the specified properties, which can be useful to improve patch performance when processing large notes dictionaries.

For MIDI clips only.

*Available since Live 11.0. Replaces get_selected_notes.*

#### `move_playing_pos`

Parameter: *beats*\
*beats* [double] relative jump distance in beats. Negative beats jump backwards. \
Jumps by given amount, unquantized. \
Unwarped audio clips, recording audio clips and recording non-overdub MIDI clips cannot jump.

#### `move_warp_marker`

Parameters: *beat_time* [double] \
*beat_time_distance* [double] \
Moves the warp marker specified by *beat_time* the specified beat time distance.

#### `quantize`

Parameter: \
*quantization_grid* [int] \
*amount* [double] \
Quantizes all notes in the clip to the quantization_grid taking the song's swing_amount into account.

#### `quantize_pitch`

Parameter: \
*pitch* [int] \
*quantization_grid* [int] \
*amount* [double] \
Same as *quantize*, but only for notes in the given pitch.

#### `remove_notes_by_id`

Parameter: \
*list* of note IDs. \
Deletes all notes associated with the provided IDs. \
Provided note IDs must be associated with existing notes in the clip. Existing notes can be queried with get_notes_extended.

*Available since Live 11.0.*

#### `remove_notes_extended`

Parameter: \
*from_pitch* [int] \
*pitch_span* [int] \
*from_time* [float] \
*time_span* [float] \
Deletes all notes that start in the given area. *from_time* and *time_span* are given in beats.

*Available since Live 11.0. Replaces remove_notes.*

#### `remove_warp_marker`

Parameter: *beat_time* [float] \
Removes the warp marker at the given beat time.

#### `scrub`

Parameter: *beat_time* [double] \
Scrub the clip to a time, specified in beats. This behaves exactly like scrubbing with the mouse; the scrub will respect Global Quantization, starting and looping in time with the transport. The scrub will continue until stop_scrub() is called.

#### `select_all_notes`

Use this function to process all notes of a clip, independent of the current selection.

Output: \
select_all_notes id 0

For MIDI clips only.

#### `select_notes_by_id`

Parameter: \
*list* of note IDs. \
Selects all notes associated with the provided IDs.

Note that this function will *not* print a warning or error if the list contains nonexistent IDs.

*Available since Live 11.0.6*

#### `set_fire_button_state`

Parameter: *state* [bool] \
If the state is set to 1, Live simulates pressing the clip start button until the state is set to 0, or until the clip is otherwise stopped.

#### `stop`

Same effect as pressing the stop button of the track, but only if this clip is actually playing or recording. If this clip is triggered or if another clip in this track is playing, it has no effect.

#### `stop_scrub`

Stops an active scrub on a clip.

## DeviceParameter

This class represents an (automatable) parameter within a MIDI or audio device. To modify a device parameter, set its value property or send its object ID to [live.remote~](https://docs.cycling74.com/max8/refpages/live.remote~).

**Canonical path:** `live_set tracks N devices M parameters L`

### Properties

#### `automation_state`

Type: int — Access: get, observe

Get the automation state of the parameter. \
0 = no automation. \
1 = automation active. \
2 = automation overridden.

#### `default_value`

Type: float — Access: get

Get the default value for this parameter. \
Only available for parameters that aren't quantized (see *is_quantized*).

#### `is_enabled`

Type: bool — Access: get

1 = the parameter value can be modified directly by the user, by sending set to a [live.object](https://docs.cycling74.com/max8/refpages/live.object), by automation or by an assigned MIDI message or keystroke. \
Parameters can be disabled because they are macro-controlled, or they are controlled by a live-remote~ object, or because Live thinks that they should not be moved.

#### `is_quantized`

Type: bool — Access: get

1 for booleans and enums \
0 for int/float parameters \
Although parameters like MidiPitch.Pitch appear quantized to the user, they actually have an is_quantized value of 0.

#### `max`

Type: float — Access: get

Largest allowed value.

#### `min`

Type: float — Access: get

Lowest allowed value.

#### `name`

Type: symbol — Access: get

The short parameter name as shown in the (closed) automation chooser.

#### `original_name`

Type: symbol — Access: get

The name of a Macro parameter before its assignment.

#### `state`

Type: int — Access: get, observe

The active state of the parameter. \
0 = the parameter is active and can be changed. \
1 = the parameter can be changed but isn't active, so changes won't have an audible effect. \
2 = the parameter cannot be changed.

#### `value`

Type: float — Access: get, set, observe

Linear-to-GUI value between min and max.

#### `value_items`

Type: StringVector — Access: get

Get a list of the possible values for this parameter. \
Only available for parameters that are quantized (see *is_quantized*).

### Functions

#### `re_enable_automation`

Re-enable automation for this parameter.

#### `str_for_value`

Parameter: *value* [float] Returns: [symbol] String representation of the specified value.

#### `__str__`

Returns: [symbol] String representation of the current parameter value.

## MixerDevice

This class represents a mixer device in Live. It provides access to volume, panning and other DeviceParameter objects. See DeviceParameter to learn how to modify them.

**Canonical path:** `live_set tracks N mixer_device`

### Children

#### `sends`

Type: list of DeviceParameter — Access: get, observe

One send per return track.

#### `cue_volume`

Type: DeviceParameter — Access: get

[in master track only]

#### `crossfader`

Type: DeviceParameter — Access: get

[in master track only]

#### `left_split_stereo`

Type: DeviceParameter — Access: get

The Track's Left Split Stereo Pan Parameter.

#### `panning`

Type: DeviceParameter — Access: get

#### `right_split_stereo`

Type: DeviceParameter — Access: get

The Track's Right Split Stereo Pan Parameter.

#### `song_tempo`

Type: DeviceParameter — Access: get

[in master track only]

#### `track_activator`

Type: DeviceParameter — Access: get

#### `volume`

Type: DeviceParameter — Access: get

### Properties

#### `crossfade_assign`

Type: int — Access: get, set, observe

0 = A, 1 = none, 2 = B [not in master track]

#### `panning_mode`

Type: int — Access: get, set, observe

Access to the Track mixer's pan mode: 0 = Stereo, 1 = Split Stereo.

## Scene

This class represents a series of clip slots in Live's Session View matrix.

**Canonical path:** `live_set scenes N`

### Children

#### `clip_slots`

Type: list of ClipSlot — Access: get, observe

### Properties

#### `color`

Type: int — Access: get, set, observe

The RGB value of the scene's color in the form 0x00rrggbb or (2^16 * red) + (2^8) * green + blue, where red, green and blue are values from 0 (dark) to 255 (light).

When setting the RGB value, the nearest color from the Scene color chooser is taken.

#### `color_index`

Type: long — Access: get, set, observe

The color index of the scene.

#### `is_empty`

Type: bool — Access: get

1 = none of the slots in the scene is filled.

#### `is_triggered`

Type: bool — Access: get, observe

1 = scene is blinking.

#### `name`

Type: symbol — Access: get, set, observe

The name of the scene.

#### `tempo`

Type: float — Access: get, set, observe

The scene's tempo. \
Returns -1 if the scene tempo is disabled.

#### `tempo_enabled`

Type: bool — Access: get, set, observe

The active state of the scene tempo. \
When disabled, the scene will use the song's tempo, \
and the tempo value returned will be -1.

#### `time_signature_numerator`

Type: int — Access: get, set, observe

The scene's time signature numerator. \
Returns -1 if the scene time signature is disabled.

#### `time_signature_denominator`

Type: int — Access: get, set, observe

The scene's time signature denominator. \
Returns -1 if the scene time signature is disabled.

#### `time_signature_enabled`

Type: bool — Access: get, set, observe

The active state of the scene time signature. \
When disabled, the scene will use the song's time signature, \
and the time signature values returned will be -1.

### Functions

#### `fire`

Parameter: force_legato (optional) [bool] \
can_select_scene_on_launch (optional) [bool] \
Fire all clip slots contained within the scene and select this scene. \
Starts recording of armed and empty tracks within a Group Track in this scene if Preferences->Launch->Start Recording on Scene Launch is ON. \
Calling with force_legato = 1 (default = 0) will launch all clips immediately in Legato, independent of their launch mode. \
When calling with can_select_scene_on_launch = 0 (default = 1) the scene is fired without selecting it.

#### `fire_as_selected`

Parameter: force_legato (optional) [bool] \
Fire the selected scene, then select the next scene. \
It doesn't matter on which scene you are calling this function. \
Calling with force_legato = 1 (default = 0) will launch all clips immediately in Legato, independent of their launch mode.

#### `set_fire_button_state`

Parameter: *state* [bool] \
If the state is set to 1, Live simulates pressing of scene button until the state is set to 0 or until the scene is stopped otherwise.

## CuePoint

Represents a locator in the Arrangement View.

**Canonical path:** `live_set cue_points N`

### Properties

#### `name`

Type: symbol — Access: get, set, observe

#### `time`

Type: float — Access: get, observe

Arrangement position of the marker in beats.

### Functions

#### `jump`

Set current Arrangement playback position to marker, quantized if song is playing.

## Other classes

One line each, copied from the page; see the source for their children, properties and functions.

- **Application** (`live_app`): This class represents the Live application. It is reachable by the root path live_app.
- **Application.View** (`live_app view`): This class represents the aspects of the Live application related to viewing the application.
- **TuningSystem** (`live_set tuning_system`): This class represents a tuning system in Live.
- **GroovePool** (`live_set groove_pool`): This class represents the groove pool in Live. It provides access to the current set's list of grooves.
- **Track.View** (`live_set tracks N view`): Representing the view aspects of a track.
- **Clip.View** (`live_set tracks N clip_slots M clip view`): Representing the view aspects of a Clip.
- **Groove** (`live_set groove_pool grooves N`): This class represents a groove in Live.

*Available since Live 11.0.*\
All grooves are stored in Live's groove pool.
- **Device** (`live_set tracks N devices M`): This class represents a MIDI or audio device in Live.
- **Device.View** (`live_set tracks N devices M view`): Representing the view aspects of a Device.
- **RackDevice** (`—`): This class represents a Live Rack Device. \
A RackDevice is a type of Device, meaning that it has all the children, properties and functions that a Device has. Listed below are members unique to RackDevice.
- **RackDevice.View** (`—`): Represents the view aspects of a Rack Device. \
A RackDevice.View is a type of Device.View, meaning that it has all the properties that a Device.View has. Listed below are the members unique to RackDevice.View.
- **DrumPad** (`live_set tracks N devices M drum_pads L`): This class represents a Drum Rack pad in Live.
- **Chain** (`live_set tracks N devices M chains L`): This class represents a group device chain in Live.
- **DrumChain** (`—`): This class represents a Drum Rack device chain in Live.

A DrumChain is a type of Chain, meaning that it has all the children, properties and functions that a Chain has. Listed below are the members unique to DrumChain.
- **ChainMixerDevice** (`live_set tracks N devices M chains L mixer_device`): This class represents a chain's mixer device in Live.
- **ShifterDevice** (`—`): This class represents an instance of the Shifter audio effect. \
A ShifterDevice is a type of device, meaning that it has all the children, properties and functions that a device has. Listed below are members unique to ShifterDevice.
- **SimplerDevice** (`—`): This class represents an instance of Simpler. \
A SimplerDevice is a type of device, meaning that it has all the children, properties and functions that a device has. Listed below are members unique to SimplerDevice.
- **SimplerDevice.View** (`—`): Represents the view aspects of a SimplerDevice. \
A SimplerDevice.View is a type of Device.View, meaning that it has all the properties that a Device.View has. Listed below are the members unique to SimplerDevice.View.
- **Sample** (`live_set tracks N devices N sample`): This class represents a sample file loaded into Simpler.
- **WavetableDevice** (`—`): This class represents a Wavetable instrument.

A WavetableDevice shares all of the children, functions and properties that a Device has. Listed below are members unique to it.
- **CompressorDevice** (`—`): This class represents a Compressor device in Live. \
A CompressorDevice shares all of the children, functions and properties of a Device; listed below are the members unique to it.
- **PluginDevice** (`—`): This class represents a plug-in device.

A PluginDevice is a type of Device, meaning that it has all the children, properties and functions that a Device has. Listed below are the members unique to PluginDevice.
- **MaxDevice** (`—`): This class represents a Max for Live device in Live.

A MaxDevice is a type of Device, meaning that it has all the children, properties and functions that a Device has. Listed below are the members unique to MaxDevice.
- **Eq8Device** (`—`): This class represents an instance of an EQ Eight device in Live. \
An Eq8Device has all the properties, functions and children of a Device. Listed below are members unique to Eq8Device.
- **Eq8Device.View** (`—`): Represents the view aspects of an Eq8Device. \
An Eq8Device.View has all the children, properties and functions of a Device.View. Listed below are members unique to it.
- **DriftDevice** (`—`): This class represents an instance of a Drift device in Live. \
A DriftDevice has all the properties, functions and children of a Device.
- **DrumCellDevice** (`—`): This class represents an instance of a Drum Sampler device in Live. \
A DrumCell has all the properties, functions and children of a Device. Listed below are members unique to DrumCell Device.
- **HybridReverbDevice** (`—`): This class represents an instance of a Hybrid Reverb device in Live. \
A HybridReverbDevice has all the properties, functions and children of a Device. Listed below are members unique to HybridReverbDevice.
- **MeldDevice** (`—`): This class represents an instance of a Meld device in Live. \
A MeldDevice has all the properties, functions and children of a Device.
- **RoarDevice** (`—`): This class represents an instance of a Roar device in Live. \
A RoarDevice has all the properties, functions and children of a Roar Device.
- **SpectralResonatorDevice** (`—`): This class represents an instance of a Spectral Resonator device in Live. \
An SpectralResonatorDevice has all the properties, functions and children of a Device. Listed below are members unique to SpectralResonatorDevice.
- **LooperDevice** (`—`): This class represents an instance of a Looper device in Live. \
An LooperDevice has all the properties, functions and children of a Device. Listed below are members unique to LooperDevice.
- **DeviceIO** (`—`): This class represents an input or output bus of a Live device.
- **ControlSurface** (`control_surfaces N`): A ControlSurface can be reached either directly by the root path control_surfaces *N* or by getting a list of active control surface IDs, via calling *get control_surfaces* on an Application object. \
The latter list is in the same order in which control surfaces appear in Live's Link/MIDI Preferences. Note the same order is not guaranteed when getting a control surface via the control_surfaces *N* path.

A control surface can be thought of as a software layer between the Live API and, in this case, Max for Live. Individiual controls on the surface are represented by objects that can be grabbed and released via Max for Live, to obtain and give back exclusive control (see *grab_control* and *release_control*). In this way, parts of the hardware can be controlled via Max for Live while other parts can retain their default functionality.

Additionally, Live offers a special *MaxForLive* control surface that has a *register_midi_control* function. Using this, Max for Live developers can set up entirely custom control surfaces by adding and grabbing arbitrary controls.
- **this_device** (`live_set tracks N devices M`): This root path represents the device containing the [live.path](https://docs.cycling74.com/max8/refpages/live.path) object to which the goto this_device message is sent. The class of this object is Device.

## See Also

- Max For Live (https://docs.cycling74.com/max8/vignettes/max_for_live_topic)
