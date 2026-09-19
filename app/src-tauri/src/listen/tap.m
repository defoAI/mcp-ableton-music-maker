// A Core Audio process tap of one process (Ableton Live), into a ring buffer.
//
// macOS 14.2 added process taps: an app can receive the audio another process
// sends to an output device, with no virtual audio driver and no change to
// that process's routing. From 14.4 the first tap raises the permission
// prompt whose text comes from NSAudioCaptureUsageDescription.
//
// Everything newer than the deployment target is weak-imported and guarded by
// @available, so the binary still launches on macOS 12 and 13 — there the
// screen says what it needs, and nothing else in the app is affected.
//
// The audio never leaves this file's ring buffer: the Rust side drains it,
// measures it and throws it away. Nothing here opens a file or a socket.

#import <Foundation/Foundation.h>
#import <CoreAudio/CoreAudio.h>
#import <AudioToolbox/AudioToolbox.h>
#include <stdatomic.h>
#include <string.h>
#include <stdlib.h>
#include <libproc.h>
#include <sys/types.h>

// Power of two: the index wrap is a mask. ~1.4 s of stereo at 48 kHz, which
// is far more than the analysis thread can fall behind between frames.
#define RING_FRAMES 65536u
#define RING_MASK (RING_FRAMES - 1u)

static float *gRing = NULL;               // interleaved stereo, RING_FRAMES * 2
static _Atomic unsigned long long gWrite = 0;      // frames written, monotonic
static unsigned long long gRead = 0;               // frames read, monotonic
static _Atomic unsigned long long gFramesSeen = 0; // every frame the tap delivered

static AudioObjectID gTap = kAudioObjectUnknown;
static AudioObjectID gAgg = kAudioObjectUnknown;
static AudioDeviceIOProcID gProc = NULL;
static dispatch_queue_t gQueue = NULL;

static double gRate = 0.0;
static unsigned int gTapChannels = 0;
static unsigned int gBufferFrames = 0;

static void set_err(char *err, int errLen, NSString *msg) {
    if (!err || errLen <= 0) return;
    const char *utf8 = [msg UTF8String];
    if (!utf8) utf8 = "unknown error";
    strncpy(err, utf8, (size_t)errLen - 1);
    err[errLen - 1] = '\0';
}

// A four-character OSStatus reads as its code; anything else as a number.
static NSString *status_text(OSStatus st) {
    char c[5] = {0};
    UInt32 be = CFSwapInt32HostToBig((UInt32)st);
    memcpy(c, &be, 4);
    BOOL printable = YES;
    for (int i = 0; i < 4; i++) {
        if (c[i] < 32 || c[i] > 126) { printable = NO; break; }
    }
    return printable ? [NSString stringWithFormat:@"'%s'", c]
                     : [NSString stringWithFormat:@"%d", (int)st];
}

// ── the ring ────────────────────────────────────────────────────────────────

// Called on the audio thread: no allocation, no locks, no logging.
static void push_frames(const AudioBufferList *list, UInt32 frames) {
    if (!gRing || frames == 0 || list == NULL || list->mNumberBuffers == 0) return;
    unsigned long long w = atomic_load_explicit(&gWrite, memory_order_relaxed);
    const AudioBuffer *b0 = &list->mBuffers[0];
    if (b0->mData == NULL) return;
    UInt32 ch0 = b0->mNumberChannels;
    if (ch0 >= 2) {
        // One interleaved buffer.
        const float *src = (const float *)b0->mData;
        UInt32 have = b0->mDataByteSize / (UInt32)sizeof(float) / ch0;
        if (frames > have) frames = have;
        for (UInt32 i = 0; i < frames; i++) {
            size_t idx = (size_t)((w + i) & RING_MASK) * 2;
            gRing[idx] = src[i * ch0];
            gRing[idx + 1] = src[i * ch0 + 1];
        }
    } else {
        // One buffer per channel; mono taps feed both sides.
        const float *l = (const float *)b0->mData;
        const float *r = l;
        if (list->mNumberBuffers > 1 && list->mBuffers[1].mData != NULL) {
            r = (const float *)list->mBuffers[1].mData;
        }
        UInt32 have = b0->mDataByteSize / (UInt32)sizeof(float);
        if (frames > have) frames = have;
        for (UInt32 i = 0; i < frames; i++) {
            size_t idx = (size_t)((w + i) & RING_MASK) * 2;
            gRing[idx] = l[i];
            gRing[idx + 1] = r[i];
        }
    }
    atomic_store_explicit(&gWrite, w + frames, memory_order_release);
    atomic_fetch_add_explicit(&gFramesSeen, frames, memory_order_relaxed);
}

// ── the C surface the Rust side calls ───────────────────────────────────────

int amm_tap_available(void) {
    if (@available(macOS 14.2, *)) {
        return 1;
    }
    return 0;
}

// Live's pid, or 0, with the name of the app bundle it runs from.
//
// Read through libproc rather than NSWorkspace: the menu bar refreshes from a
// background thread, and AppKit is not the thing to call from one.
int amm_find_live_pid(char *nameOut, int nameLen) {
    int bytes = proc_listpids(PROC_ALL_PIDS, 0, NULL, 0);
    if (bytes <= 0) return 0;
    int capacity = (int)(bytes / (int)sizeof(pid_t)) + 64;
    pid_t *pids = (pid_t *)calloc((size_t)capacity, sizeof(pid_t));
    if (!pids) return 0;

    int found = 0;
    bytes = proc_listpids(PROC_ALL_PIDS, 0, pids, (int)((size_t)capacity * sizeof(pid_t)));
    int count = bytes > 0 ? bytes / (int)sizeof(pid_t) : 0;
    char path[PROC_PIDPATHINFO_MAXSIZE];
    for (int i = 0; i < count; i++) {
        if (pids[i] <= 0) continue;
        if (proc_pidpath(pids[i], path, sizeof(path)) <= 0) continue;
        // ".../Ableton Live 12 Suite.app/Contents/MacOS/Live"
        const char *app = strstr(path, "/Ableton Live");
        if (!app || !strstr(path, ".app/Contents/MacOS/")) continue;
        found = (int)pids[i];
        if (nameOut && nameLen > 0) {
            const char *start = app + 1;
            const char *dot = strstr(start, ".app/");
            size_t len = dot ? (size_t)(dot - start) : strlen(start);
            if (len > (size_t)nameLen - 1) len = (size_t)nameLen - 1;
            memcpy(nameOut, start, len);
            nameOut[len] = '\0';
        }
        break;
    }
    free(pids);
    return found;
}

void amm_tap_stop(void) {
    if (@available(macOS 14.2, *)) {
        if (gAgg != kAudioObjectUnknown && gProc != NULL) {
            AudioDeviceStop(gAgg, gProc);
            AudioDeviceDestroyIOProcID(gAgg, gProc);
        }
        if (gAgg != kAudioObjectUnknown) {
            AudioHardwareDestroyAggregateDevice(gAgg);
        }
        if (gTap != kAudioObjectUnknown) {
            AudioHardwareDestroyProcessTap(gTap);
        }
    }
    gProc = NULL;
    gAgg = kAudioObjectUnknown;
    gTap = kAudioObjectUnknown;
    gQueue = NULL;
    if (gRing) {
        free(gRing);
        gRing = NULL;
    }
    atomic_store(&gWrite, 0);
    atomic_store(&gFramesSeen, 0);
    gRead = 0;
    gRate = 0.0;
    gTapChannels = 0;
    gBufferFrames = 0;
}

int amm_tap_start(int pid, char *err, int errLen) {
    if (@available(macOS 14.2, *)) {
        // supported; fall through
    } else {
        set_err(err, errLen,
                @"This screen needs macOS 14.4 or later. Everything else in the app works here.");
        return -1;
    }

    if (@available(macOS 14.2, *)) {
        @autoreleasepool {
            amm_tap_stop();

            // 1. The audio object that stands for Live's process.
            AudioObjectPropertyAddress pidAddr = {
                kAudioHardwarePropertyTranslatePIDToProcessObject,
                kAudioObjectPropertyScopeGlobal,
                kAudioObjectPropertyElementMain,
            };
            pid_t wanted = (pid_t)pid;
            AudioObjectID processObject = kAudioObjectUnknown;
            UInt32 size = (UInt32)sizeof(processObject);
            OSStatus st = AudioObjectGetPropertyData(kAudioObjectSystemObject, &pidAddr,
                                                     (UInt32)sizeof(wanted), &wanted,
                                                     &size, &processObject);
            if (st != noErr || processObject == kAudioObjectUnknown) {
                set_err(err, errLen,
                        [NSString stringWithFormat:
                            @"Core Audio does not know process %d yet (%@). Play something in Live once, then try again.",
                            pid, status_text(st)]);
                return -2;
            }

            // 2. A stereo mixdown tap of that one process, left unmuted so Live
            //    keeps playing through whatever output it had.
            CATapDescription *desc =
                [[CATapDescription alloc] initStereoMixdownOfProcesses:@[ @(processObject) ]];
            if (desc == nil) {
                set_err(err, errLen, @"Could not describe the tap.");
                return -3;
            }
            desc.name = @"Ableton Music Maker Listen";
            desc.muteBehavior = CATapUnmuted;

            AudioObjectID tapID = kAudioObjectUnknown;
            st = AudioHardwareCreateProcessTap(desc, &tapID);
            if (st != noErr || tapID == kAudioObjectUnknown) {
                set_err(err, errLen,
                        [NSString stringWithFormat:
                            @"macOS refused the tap (%@). Allow “Ableton Music Maker” under System Settings › Privacy & Security › Screen & System Audio Recording.",
                            status_text(st)]);
                return -4;
            }
            gTap = tapID;

            // The tap's own format tells us the rate and channel count.
            AudioObjectPropertyAddress fmtAddr = {
                kAudioTapPropertyFormat,
                kAudioObjectPropertyScopeGlobal,
                kAudioObjectPropertyElementMain,
            };
            AudioStreamBasicDescription asbd;
            memset(&asbd, 0, sizeof(asbd));
            size = (UInt32)sizeof(asbd);
            if (AudioObjectGetPropertyData(gTap, &fmtAddr, 0, NULL, &size, &asbd) == noErr) {
                gRate = asbd.mSampleRate;
                gTapChannels = asbd.mChannelsPerFrame;
            }

            // 3. A private aggregate device that carries only this tap. Private
            //    means no other app sees it in its device list.
            NSString *tapUID = [desc.UUID UUIDString];
            NSDictionary *aggDesc = @{
                @kAudioAggregateDeviceNameKey: @"Ableton Music Maker Listen",
                @kAudioAggregateDeviceUIDKey: [[NSUUID UUID] UUIDString],
                @kAudioAggregateDeviceIsPrivateKey: @(1),
                @kAudioAggregateDeviceIsStackedKey: @(0),
                @kAudioAggregateDeviceTapAutoStartKey: @(1),
                @kAudioAggregateDeviceSubDeviceListKey: @[],
                @kAudioAggregateDeviceTapListKey: @[ @{
                    @kAudioSubTapUIDKey: tapUID,
                    @kAudioSubTapDriftCompensationKey: @(1),
                } ],
            };
            AudioObjectID aggID = kAudioObjectUnknown;
            st = AudioHardwareCreateAggregateDevice((__bridge CFDictionaryRef)aggDesc, &aggID);
            if (st != noErr || aggID == kAudioObjectUnknown) {
                set_err(err, errLen,
                        [NSString stringWithFormat:@"Could not build the private audio device (%@).",
                                                   status_text(st)]);
                amm_tap_stop();
                return -5;
            }
            gAgg = aggID;

            if (gRate <= 0.0) {
                AudioObjectPropertyAddress rateAddr = {
                    kAudioDevicePropertyNominalSampleRate,
                    kAudioObjectPropertyScopeGlobal,
                    kAudioObjectPropertyElementMain,
                };
                Float64 rate = 0;
                size = (UInt32)sizeof(rate);
                if (AudioObjectGetPropertyData(gAgg, &rateAddr, 0, NULL, &size, &rate) == noErr) {
                    gRate = rate;
                }
            }
            AudioObjectPropertyAddress bufAddr = {
                kAudioDevicePropertyBufferFrameSize,
                kAudioObjectPropertyScopeGlobal,
                kAudioObjectPropertyElementMain,
            };
            UInt32 bufFrames = 0;
            size = (UInt32)sizeof(bufFrames);
            if (AudioObjectGetPropertyData(gAgg, &bufAddr, 0, NULL, &size, &bufFrames) == noErr) {
                gBufferFrames = bufFrames;
            }

            // 4. The ring, then the IO block that fills it.
            gRing = (float *)calloc((size_t)RING_FRAMES * 2, sizeof(float));
            if (!gRing) {
                set_err(err, errLen, @"Out of memory for the audio buffer.");
                amm_tap_stop();
                return -6;
            }

            gQueue = dispatch_queue_create("com.defoai.ableton-music-maker.listen",
                                           DISPATCH_QUEUE_SERIAL);
            AudioDeviceIOProcID procID = NULL;
            st = AudioDeviceCreateIOProcIDWithBlock(
                &procID, gAgg, gQueue,
                ^(const AudioTimeStamp *inNow, const AudioBufferList *inInputData,
                  const AudioTimeStamp *inInputTime, AudioBufferList *outOutputData,
                  const AudioTimeStamp *inOutputTime) {
                    (void)inNow;
                    (void)inInputTime;
                    (void)outOutputData;
                    (void)inOutputTime;
                    if (inInputData == NULL || inInputData->mNumberBuffers == 0) return;
                    const AudioBuffer *b0 = &inInputData->mBuffers[0];
                    UInt32 ch = b0->mNumberChannels ? b0->mNumberChannels : 1;
                    UInt32 frames = b0->mDataByteSize / (UInt32)sizeof(float) / ch;
                    push_frames(inInputData, frames);
                });
            if (st != noErr || procID == NULL) {
                set_err(err, errLen,
                        [NSString stringWithFormat:@"Could not start reading the tap (%@).",
                                                   status_text(st)]);
                amm_tap_stop();
                return -7;
            }
            gProc = procID;

            st = AudioDeviceStart(gAgg, gProc);
            if (st != noErr) {
                set_err(err, errLen,
                        [NSString stringWithFormat:@"Could not start the audio device (%@).",
                                                   status_text(st)]);
                amm_tap_stop();
                return -8;
            }
            return 0;
        }
    }
    set_err(err, errLen, @"This screen needs macOS 14.4 or later.");
    return -1;
}

int amm_tap_format(double *rate, int *channels, unsigned int *bufferFrames) {
    if (rate) *rate = gRate;
    if (channels) *channels = (int)gTapChannels;
    if (bufferFrames) *bufferFrames = gBufferFrames;
    return gRing != NULL ? 1 : 0;
}

// Copy up to maxFrames of the newest audio out of the ring. A reader that fell
// behind gets the newest frames and drops the rest: this feeds a meter, not a
// recording.
int amm_tap_drain(float *left, float *right, int maxFrames) {
    if (!gRing || !left || !right || maxFrames <= 0) return 0;
    unsigned long long w = atomic_load_explicit(&gWrite, memory_order_acquire);
    unsigned long long r = gRead;
    if (w <= r) return 0;
    unsigned long long avail = w - r;
    if (avail > (unsigned long long)RING_FRAMES) {
        r = w - RING_FRAMES;
        avail = RING_FRAMES;
    }
    if (avail > (unsigned long long)maxFrames) {
        r = w - (unsigned long long)maxFrames;
        avail = (unsigned long long)maxFrames;
    }
    for (unsigned long long i = 0; i < avail; i++) {
        size_t idx = (size_t)((r + i) & RING_MASK) * 2;
        left[i] = gRing[idx];
        right[i] = gRing[idx + 1];
    }
    gRead = r + avail;
    return (int)avail;
}

// How many audio devices the system has. The private aggregate device the
// tap needs must be gone once listening stops, so the probe compares this
// before and after.
unsigned int amm_audio_device_count(void) {
    AudioObjectPropertyAddress addr = {
        kAudioHardwarePropertyDevices,
        kAudioObjectPropertyScopeGlobal,
        kAudioObjectPropertyElementMain,
    };
    UInt32 size = 0;
    if (AudioObjectGetPropertyDataSize(kAudioObjectSystemObject, &addr, 0, NULL, &size) != noErr) {
        return 0;
    }
    return (unsigned int)(size / sizeof(AudioObjectID));
}

unsigned long long amm_tap_frames_seen(void) {
    return atomic_load_explicit(&gFramesSeen, memory_order_relaxed);
}
