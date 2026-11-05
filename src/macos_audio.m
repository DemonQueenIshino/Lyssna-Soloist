#import <AVFoundation/AVFoundation.h>
#import <AudioToolbox/AudioToolbox.h>
#import <CoreAudio/CoreAudio.h>
#import <CoreMedia/CoreMedia.h>

// The renderer keeps Control Center in charge of Fixed / Head Tracked / Off.
@interface LyssnaAudioOutput : NSObject
@property(nonatomic, strong) AVSampleBufferAudioRenderer *renderer;
@property(nonatomic, strong) AVSampleBufferRenderSynchronizer *synchronizer;
@property(nonatomic) CMAudioFormatDescriptionRef format;
@property(nonatomic) CMTime nextTime;
@property(nonatomic) CMTime startHostTime;
@property(nonatomic) BOOL started;
@end

@implementation LyssnaAudioOutput
- (void)dealloc {
    [_synchronizer setRate:0.0];
    [_renderer flush];
    if (_format) CFRelease(_format);
}
@end

static NSString *deviceUID(NSString *name) {
    AudioObjectPropertyAddress address = {
        kAudioHardwarePropertyDevices, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain
    };
    UInt32 size = 0;
    if (AudioObjectGetPropertyDataSize(kAudioObjectSystemObject, &address, 0, NULL, &size) != noErr) return nil;
    AudioDeviceID *devices = malloc(size);
    if (!devices) return nil;
    NSString *result = nil;
    if (AudioObjectGetPropertyData(kAudioObjectSystemObject, &address, 0, NULL, &size, devices) == noErr) {
        for (UInt32 index = 0; index < size / sizeof(AudioDeviceID); index++) {
            AudioObjectPropertyAddress streams = {
                kAudioDevicePropertyStreams, kAudioObjectPropertyScopeOutput, kAudioObjectPropertyElementMain
            };
            UInt32 streamSize = 0;
            if (AudioObjectGetPropertyDataSize(devices[index], &streams, 0, NULL, &streamSize) != noErr || !streamSize) continue;
            AudioObjectPropertyAddress property = {
                kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain
            };
            CFStringRef value = NULL;
            UInt32 length = sizeof(value);
            if (AudioObjectGetPropertyData(devices[index], &property, 0, NULL, &length, &value) != noErr) continue;
            BOOL matches = [(__bridge NSString *)value isEqualToString:name];
            if (value) CFRelease(value);
            if (!matches) continue;
            property.mSelector = kAudioDevicePropertyDeviceUID;
            length = sizeof(value);
            value = NULL;
            if (AudioObjectGetPropertyData(devices[index], &property, 0, NULL, &length, &value) == noErr) {
                result = CFBridgingRelease(value);
            }
            break;
        }
    }
    free(devices);
    return result;
}

void *lyssna_audio_create(const char *name) {
    @autoreleasepool {
        if (@available(macOS 12.0, *)) {
            LyssnaAudioOutput *output = [LyssnaAudioOutput new];
            output.renderer = [AVSampleBufferAudioRenderer new];
            output.renderer.allowedAudioSpatializationFormats = AVAudioSpatializationFormatMonoAndStereo;
            if (name) {
                NSString *uid = deviceUID([NSString stringWithUTF8String:name]);
                if (!uid) return NULL;
                output.renderer.audioOutputDeviceUniqueID = uid;
            }
            output.synchronizer = [AVSampleBufferRenderSynchronizer new];
            // Our short queue must not wait for AVFoundation's larger preroll.
            output.synchronizer.delaysRateChangeUntilHasSufficientMediaData = NO;
            [output.synchronizer addRenderer:output.renderer];
            output.nextTime = kCMTimeZero;
            AudioStreamBasicDescription description = {
                .mSampleRate = 44100, .mFormatID = kAudioFormatLinearPCM,
                .mFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked | kAudioFormatFlagsNativeEndian,
                .mBytesPerPacket = 8, .mFramesPerPacket = 1, .mBytesPerFrame = 8,
                .mChannelsPerFrame = 2, .mBitsPerChannel = 32
            };
            AudioChannelLayout layout = { .mChannelLayoutTag = kAudioChannelLayoutTag_Stereo };
            CMAudioFormatDescriptionRef format = NULL;
            OSStatus status = CMAudioFormatDescriptionCreate(kCFAllocatorDefault, &description,
                sizeof(layout), &layout, 0, NULL, NULL, &format);
            if (status != noErr) return NULL;
            output.format = format;
            return (__bridge_retained void *)output;
        }
        return NULL;
    }
}

int lyssna_audio_ready(void *handle) {
    @autoreleasepool {
        LyssnaAudioOutput *output = (__bridge LyssnaAudioOutput *)handle;
        if (output.renderer.status == AVQueuedSampleBufferRenderingStatusFailed) return -1;
        if (!output.started) return output.renderer.readyForMoreMediaData ? 1 : 0;
        // Pace against the same host clock used to anchor playback. The render
        // clock can lag behind the audio queue while AirPods spatialization
        // starts or changes mode. A 30 ms lead caused every buffer to be late.
        CMTime now = CMTimeSubtract(CMClockGetTime(CMClockGetHostTimeClock()), output.startHostTime);
        CMTime ahead = CMTimeSubtract(output.nextTime, now);
        return output.renderer.readyForMoreMediaData && CMTimeGetSeconds(ahead) < 0.25 ? 1 : 0;
    }
}

bool lyssna_audio_enqueue(void *handle, const float *samples, size_t frames) {
    @autoreleasepool {
        LyssnaAudioOutput *output = (__bridge LyssnaAudioOutput *)handle;
        CMTime hostNow = CMClockGetTime(CMClockGetHostTimeClock());
        if (!output.started) {
            output.startHostTime = CMTimeAdd(hostNow, CMTimeMake(1, 4));
        } else {
            // Recover from a scheduling stall with usable lead time instead of
            // repeatedly enqueueing at the already missed presentation deadline.
            CMTime now = CMTimeSubtract(hostNow, output.startHostTime);
            if (CMTimeCompare(output.nextTime, CMTimeAdd(now, CMTimeMake(1, 20))) < 0) {
                output.nextTime = CMTimeAdd(now, CMTimeMake(1, 4));
            }
        }
        CMBlockBufferRef block = NULL;
        OSStatus status = CMBlockBufferCreateWithMemoryBlock(kCFAllocatorDefault, NULL, frames * 8,
            kCFAllocatorDefault, NULL, 0, frames * 8, 0, &block);
        if (status != noErr) return false;
        status = CMBlockBufferReplaceDataBytes(samples, block, 0, frames * 8);
        CMSampleBufferRef buffer = NULL;
        CMSampleTimingInfo timing = {
            .duration = CMTimeMake(1, 44100), .presentationTimeStamp = output.nextTime,
            .decodeTimeStamp = kCMTimeInvalid
        };
        size_t sampleSize = 8;
        if (status == noErr) {
            status = CMSampleBufferCreateReady(kCFAllocatorDefault, block, output.format, frames,
                1, &timing, 1, &sampleSize, &buffer);
        }
        CFRelease(block);
        if (status != noErr) return false;
        [output.renderer enqueueSampleBuffer:buffer];
        CFRelease(buffer);
        output.nextTime = CMTimeAdd(output.nextTime, CMTimeMake(frames, 44100));
        if (!output.started) {
            [output.synchronizer setRate:1.0 time:kCMTimeZero atHostTime:output.startHostTime];
            output.started = YES;
        }
        return output.renderer.status != AVQueuedSampleBufferRenderingStatusFailed;
    }
}

void lyssna_audio_destroy(void *handle) {
    @autoreleasepool {
        LyssnaAudioOutput *output = CFBridgingRelease(handle);
        [output.synchronizer setRate:0.0];
        [output.renderer flush];
    }
}
