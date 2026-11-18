// Native frame access only. All codecs come from the macOS frameworks.
#import <AVFoundation/AVFoundation.h>
#import <CoreVideo/CoreVideo.h>
#include <math.h>
#include <stdint.h>
#include <string.h>
#include <stdio.h>

// Kept in the same order as Rust's repr(C) NativeInfo.
typedef struct {
    uint32_t width, height, frames, fps_num, fps_den;
} H2MovieInfo;

@interface H2MovieReader : NSObject
@property(nonatomic, strong) AVURLAsset *asset;
@property(nonatomic, strong) AVAssetTrack *track;
@property(nonatomic, strong) AVAssetReader *reader;
@property(nonatomic, strong) AVAssetReaderTrackOutput *output;
@property(nonatomic) uint32_t nextFrame;
@property(nonatomic) H2MovieInfo info;
// The sound track, read on its own reader so video rewinds leave it alone.
@property(nonatomic, strong) AVAssetTrack *audioTrack;
@property(nonatomic, strong) AVAssetReader *audioReader;
@property(nonatomic, strong) AVAssetReaderTrackOutput *audioOutput;
// Decoded bytes not yet handed out (sample buffers do not end on our reads).
@property(nonatomic, strong) NSMutableData *audioPending;
@property(nonatomic) NSUInteger audioPendingAt;
@property(nonatomic) uint32_t audioRate;
@property(nonatomic) BOOL audioEnded;
- (BOOL)reset;
- (BOOL)resetAudio;
@end

@implementation H2MovieReader
- (BOOL)reset {
    [self.reader cancelReading];
    self.reader = [[AVAssetReader alloc] initWithAsset:self.asset error:nil];
    self.output = [[AVAssetReaderTrackOutput alloc] initWithTrack:self.track
        outputSettings:@{(id)kCVPixelBufferPixelFormatTypeKey: @(kCVPixelFormatType_32BGRA)}];
    self.output.alwaysCopiesSampleData = NO;
    if (!self.reader || ![self.reader canAddOutput:self.output]) return NO;
    [self.reader addOutput:self.output];
    self.nextFrame = 0;
    return [self.reader startReading];
}
// Interleaved signed 16-bit stereo at the track's own rate; Core Audio
// converts from AAC and mixes other channel counts to two.
- (BOOL)resetAudio {
    [self.audioReader cancelReading];
    self.audioPending = [NSMutableData data];
    self.audioPendingAt = 0;
    self.audioEnded = NO;
    if (!self.audioTrack || !self.audioRate) return NO;
    self.audioReader = [[AVAssetReader alloc] initWithAsset:self.asset error:nil];
    NSDictionary *settings = @{
        AVFormatIDKey: @(kAudioFormatLinearPCM),
        AVSampleRateKey: @(self.audioRate),
        AVNumberOfChannelsKey: @2,
        AVLinearPCMBitDepthKey: @16,
        AVLinearPCMIsFloatKey: @NO,
        AVLinearPCMIsBigEndianKey: @NO,
        AVLinearPCMIsNonInterleaved: @NO,
    };
    self.audioOutput = [[AVAssetReaderTrackOutput alloc] initWithTrack:self.audioTrack outputSettings:settings];
    if (!self.audioReader || ![self.audioReader canAddOutput:self.audioOutput]) return NO;
    [self.audioReader addOutput:self.audioOutput];
    return [self.audioReader startReading];
}
- (void)dealloc { [_reader cancelReading]; [_audioReader cancelReading]; }
@end

void *h2_movie_open(const char *path, H2MovieInfo *info, char *error, size_t errorLen) {
#define FAIL(reason) do { snprintf(error, errorLen, "%s", reason); return NULL; } while (0)
    @autoreleasepool {
        NSString *name = [[NSFileManager defaultManager] stringWithFileSystemRepresentation:path length:strlen(path)];
        if (!name) FAIL("invalid movie path");
        H2MovieReader *movie = [H2MovieReader new];
        movie.asset = [AVURLAsset URLAssetWithURL:[NSURL fileURLWithPath:name] options:@{AVURLAssetPreferPreciseDurationAndTimingKey: @YES}];
        movie.track = [[movie.asset tracksWithMediaType:AVMediaTypeVideo] firstObject];
        if (!movie.track) FAIL("movie has no readable video track");
        // Read compressed sample metadata once to count actual frames, rather
        // than rounding duration * fps (which can be off by one at 30000/1001).
        AVAssetReader *counter = [[AVAssetReader alloc] initWithAsset:movie.asset error:nil];
        AVAssetReaderTrackOutput *packets = [[AVAssetReaderTrackOutput alloc] initWithTrack:movie.track outputSettings:nil];
        if (!counter || ![counter canAddOutput:packets]) FAIL("cannot create AVAssetReader metadata output");
        [counter addOutput:packets];
        if (![counter startReading]) FAIL(counter.error.description.UTF8String ?: "metadata reader failed");
        uint64_t frames = 0;
        CMSampleBufferRef sample;
        // The rate is the first sample's duration. track.minFrameDuration is
        // only a fallback: muxers round it differently from the samples, so
        // comparing against it rejects ordinary constant-rate files. The last
        // sample may be shorter (it is trimmed to the stream end); any other
        // difference means variable-rate video.
        CMTime duration = kCMTimeInvalid, shortLast = kCMTimeInvalid;
        uint64_t shortLastAt = 0;
        while ((sample = [packets copyNextSampleBuffer])) {
            CMTime sampleDuration = CMSampleBufferGetDuration(sample);
            if (CMTIME_IS_NUMERIC(sampleDuration) && sampleDuration.value > 0) {
                if (!CMTIME_IS_NUMERIC(duration)) duration = sampleDuration;
                else if (CMTimeCompare(duration, sampleDuration) != 0) {
                    if (CMTIME_IS_NUMERIC(shortLast) || CMTimeCompare(sampleDuration, duration) > 0) {
                        CFRelease(sample); [counter cancelReading]; FAIL("variable-rate video is unsupported");
                    }
                    shortLast = sampleDuration;
                    shortLastAt = frames;
                }
            }
            frames += CMSampleBufferGetNumSamples(sample);
            CFRelease(sample);
            if (frames > UINT32_MAX) { [counter cancelReading]; FAIL("too many movie frames"); }
        }
        if (counter.status != AVAssetReaderStatusCompleted || !frames) FAIL(counter.error.description.UTF8String ?: "no video samples");
        if (CMTIME_IS_NUMERIC(shortLast) && shortLastAt + 1 != frames) FAIL("variable-rate video is unsupported");
        if (!CMTIME_IS_NUMERIC(duration)) duration = movie.track.minFrameDuration;
        // The installer preserves constant-rate Bink timing. Reject variable
        // or unknown rates rather than inventing an HBINK timebase.
        if (!CMTIME_IS_NUMERIC(duration) || duration.value <= 0 || duration.timescale <= 0 ||
            duration.value > UINT32_MAX) FAIL("video sample has no valid duration");
        CGSize size = movie.track.naturalSize;
        if (!isfinite(size.width) || !isfinite(size.height) || size.width < 1 || size.height < 1 ||
            size.width > 4096 || size.height > 4096) FAIL("unsupported movie dimensions");
        H2MovieInfo metadata = {(uint32_t)size.width, (uint32_t)size.height, (uint32_t)frames,
            (uint32_t)duration.timescale, (uint32_t)duration.value};
        movie.info = metadata;
        if (![movie reset]) FAIL(movie.reader.error.description.UTF8String ?: "cannot start BGRA reader");
        // Sound is optional: a movie without a readable track plays silent.
        movie.audioTrack = [[movie.asset tracksWithMediaType:AVMediaTypeAudio] firstObject];
        if (movie.audioTrack) {
            CMAudioFormatDescriptionRef desc =
                (__bridge CMAudioFormatDescriptionRef)movie.audioTrack.formatDescriptions.firstObject;
            const AudioStreamBasicDescription *asbd = desc ? CMAudioFormatDescriptionGetStreamBasicDescription(desc) : NULL;
            if (asbd && isfinite(asbd->mSampleRate) && asbd->mSampleRate >= 8000 && asbd->mSampleRate <= 192000)
                movie.audioRate = (uint32_t)asbd->mSampleRate;
            if (![movie resetAudio]) movie.audioRate = 0;
        }
        *info = metadata;
        return (__bridge_retained void *)movie;
    }
}

#undef FAIL

// Calls are serialized by Rust's &mut MovieStream. Rewinds restart reading;
// the normal path uses a single sequential decoder and bounded frame storage.
// A rewind (or seek backwards) is O(n): the reader is recreated and decodes
// forward from frame 0 to the requested frame. Fine for looping; do not use
// it for random access.
int h2_movie_decode(void *handle, uint32_t index, uint8_t *dest, size_t len) {
    @autoreleasepool {
        H2MovieReader *movie = (__bridge H2MovieReader *)handle;
        H2MovieInfo info = movie.info;
        if (index >= info.frames || len != (size_t)info.width * info.height * 4) return 0;
        if (index < movie.nextFrame && ![movie reset]) return 0;
        while (movie.nextFrame <= index) {
            CMSampleBufferRef sample = [movie.output copyNextSampleBuffer];
            if (!sample) {
                // Mid-stream error: the reader is dead. Force a reset on the
                // next call instead of staying stuck at the failure point.
                movie.nextFrame = UINT32_MAX;
                return 0;
            }
            uint32_t current = movie.nextFrame++;
            if (current != index) { CFRelease(sample); continue; }
            CVPixelBufferRef pixel = CMSampleBufferGetImageBuffer(sample);
            if (!pixel || CVPixelBufferGetPixelFormatType(pixel) != kCVPixelFormatType_32BGRA ||
                CVPixelBufferGetWidth(pixel) != info.width || CVPixelBufferGetHeight(pixel) != info.height ||
                CVPixelBufferLockBaseAddress(pixel, kCVPixelBufferLock_ReadOnly) != kCVReturnSuccess) {
                CFRelease(sample); return 0;
            }
            const uint8_t *base = CVPixelBufferGetBaseAddress(pixel);
            size_t pitch = CVPixelBufferGetBytesPerRow(pixel);
            int success = base != NULL && pitch >= (size_t)info.width * 4;
            if (success) {
                for (uint32_t y = 0; y < info.height; ++y)
                    memcpy(dest + (size_t)y * info.width * 4, base + (size_t)y * pitch, (size_t)info.width * 4);
            }
            CVPixelBufferUnlockBaseAddress(pixel, kCVPixelBufferLock_ReadOnly);
            CFRelease(sample);
            return success;
        }
        return 0;
    }
}
// The sound track's rate (two channels), or 0 without one.
uint32_t h2_movie_audio_rate(void *handle) {
    H2MovieReader *movie = (__bridge H2MovieReader *)handle;
    return movie.audioRate;
}

// Up to `frames` stereo frames in order; 0 at the end or on error.
size_t h2_movie_audio_read(void *handle, int16_t *dest, size_t frames) {
    @autoreleasepool {
        H2MovieReader *movie = (__bridge H2MovieReader *)handle;
        if (!movie.audioRate || !movie.audioPending) return 0;
        const size_t frameBytes = 4;
        size_t want = frames * frameBytes, have = 0;
        uint8_t *out = (uint8_t *)dest;
        while (have < want) {
            NSUInteger left = movie.audioPending.length - movie.audioPendingAt;
            if (left > 0) {
                size_t n = MIN((size_t)left, want - have);
                memcpy(out + have, (const uint8_t *)movie.audioPending.bytes + movie.audioPendingAt, n);
                movie.audioPendingAt += n;
                have += n;
                continue;
            }
            if (movie.audioEnded) break;
            CMSampleBufferRef sample = [movie.audioOutput copyNextSampleBuffer];
            if (!sample) { movie.audioEnded = YES; break; }
            CMBlockBufferRef block = CMSampleBufferGetDataBuffer(sample);
            size_t len = block ? CMBlockBufferGetDataLength(block) : 0;
            NSMutableData *data = [NSMutableData dataWithLength:len];
            if (len && CMBlockBufferCopyDataBytes(block, 0, len, data.mutableBytes) != kCMBlockBufferNoErr) {
                CFRelease(sample); movie.audioEnded = YES; break;
            }
            CFRelease(sample);
            movie.audioPending = data;
            movie.audioPendingAt = 0;
        }
        return have / frameBytes;
    }
}

int h2_movie_audio_rewind(void *handle) {
    @autoreleasepool {
        H2MovieReader *movie = (__bridge H2MovieReader *)handle;
        return movie.audioRate && [movie resetAudio];
    }
}

void h2_movie_close(void *handle) {
    @autoreleasepool {
        H2MovieReader *movie = (__bridge_transfer H2MovieReader *)handle;
        [movie.reader cancelReading];
        [movie.audioReader cancelReading];
    }
}
