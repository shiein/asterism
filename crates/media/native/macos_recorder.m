#import "macos_recorder.h"

#import <AVFoundation/AVFoundation.h>
#import <AudioToolbox/AudioToolbox.h>
#import <CoreGraphics/CoreGraphics.h>
#import <CoreMedia/CoreMedia.h>
#import <CoreVideo/CoreVideo.h>
#import <Foundation/Foundation.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>

#include <stdatomic.h>
#include <string.h>

static void set_err(char *err, int errlen, NSString *msg) {
    if (!err || errlen <= 0) {
        return;
    }
    const char *utf8 = msg.UTF8String ?: "unknown error";
    strncpy(err, utf8, (size_t)errlen - 1);
    err[errlen - 1] = 0;
}

int asterism_macos_screen_access_ok(void) {
    if (@available(macOS 11.0, *)) {
        return CGPreflightScreenCaptureAccess() ? 1 : 0;
    }
    return 1;
}

int asterism_macos_request_screen_access(void) {
    if (@available(macOS 11.0, *)) {
        return CGRequestScreenCaptureAccess() ? 1 : 0;
    }
    return 1;
}

int asterism_macos_mic_access_ok(void) {
    AVAuthorizationStatus st =
        [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio];
    return st == AVAuthorizationStatusAuthorized ? 1 : 0;
}

void asterism_macos_request_mic_access(void) {
    [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio completionHandler:^(BOOL granted) {
        (void)granted;
    }];
}

@interface AsterismAudioSink : NSObject <SCStreamOutput, SCStreamDelegate>
@property(nonatomic, assign) ExtAudioFileRef extFile;
@property(nonatomic, assign) BOOL running;
@end

@implementation AsterismAudioSink
- (void)stream:(SCStream *)stream
    didOutputSampleBuffer:(CMSampleBufferRef)sampleBuffer
                    ofType:(SCStreamOutputType)type {
    (void)stream;
    if (type != SCStreamOutputTypeAudio || !self.extFile || !self.running) {
        return;
    }
    CMBlockBufferRef block = CMSampleBufferGetDataBuffer(sampleBuffer);
    if (!block) {
        return;
    }
    size_t len = 0;
    char *data = NULL;
    if (CMBlockBufferGetDataPointer(block, 0, NULL, &len, &data) != kCMBlockBufferNoErr || !data) {
        return;
    }
    const AudioStreamBasicDescription *asbd = NULL;
    CMAudioFormatDescriptionRef fmt = CMSampleBufferGetFormatDescription(sampleBuffer);
    if (fmt) {
        asbd = CMAudioFormatDescriptionGetStreamBasicDescription(fmt);
    }
    if (!asbd) {
        return;
    }
    UInt32 frames = (UInt32)(len / (asbd->mBytesPerFrame ? asbd->mBytesPerFrame : 4));
    ExtAudioFileWrite(self.extFile, frames, &(AudioBufferList){
        .mNumberBuffers = 1,
        .mBuffers[0] = {.mNumberChannels = asbd->mChannelsPerFrame, .mDataByteSize = (UInt32)len, .mData = data},
    });
}

- (void)stream:(SCStream *)stream didStopWithError:(NSError *)error {
    (void)stream;
    (void)error;
    self.running = NO;
}
@end

struct AsterismMacRecorder {
    AVAssetWriter *writer;
    AVAssetWriterInput *videoIn;
    AVAssetWriterInputPixelBufferAdaptor *adaptor;
    AVAudioRecorder *mic;
    SCStream *sysStream;
    AsterismAudioSink *sysSink;
    NSURL *videoURL;
    NSURL *micURL;
    NSURL *sysURL;
    NSURL *outputURL;
    int width;
    int height;
    int fps;
    int audioMode;
    int started;
};

static NSURL *temp_url(NSString *ext) {
    NSString *name = [NSString stringWithFormat:@"asterism-%@.%@", NSUUID.UUID.UUIDString, ext];
    return [NSURL fileURLWithPath:[NSTemporaryDirectory() stringByAppendingPathComponent:name]];
}

static AVAssetTrack *first_track(AVAsset *asset, AVMediaType type) {
    if (@available(macOS 12.0, *)) {
        __block NSArray<AVAssetTrack *> *tracks = nil;
        dispatch_semaphore_t sem = dispatch_semaphore_create(0);
        [asset loadTracksWithMediaType:type
                     completionHandler:^(NSArray<AVAssetTrack *> *loaded, NSError *error) {
                         (void)error;
                         tracks = loaded;
                         dispatch_semaphore_signal(sem);
                     }];
        dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC));
        return tracks.firstObject;
    }
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
    return [[asset tracksWithMediaType:type] firstObject];
#pragma clang diagnostic pop
}

static int mux_tracks(NSURL *video, NSURL *mic, NSURL *sysAudio, NSURL *outURL, NSError **err) {
    AVMutableComposition *comp = [AVMutableComposition composition];
    AVURLAsset *vAsset = [AVURLAsset URLAssetWithURL:video options:nil];
    AVAssetTrack *vTrack = first_track(vAsset, AVMediaTypeVideo);
    if (!vTrack) {
        return -1;
    }
    CMTime duration = vAsset.duration;
    AVMutableCompositionTrack *cv =
        [comp addMutableTrackWithMediaType:AVMediaTypeVideo preferredTrackID:kCMPersistentTrackID_Invalid];
    if (![cv insertTimeRange:CMTimeRangeMake(kCMTimeZero, duration) ofTrack:vTrack atTime:kCMTimeZero error:err]) {
        return -1;
    }
    void (^addAudio)(NSURL *) = ^(NSURL *url) {
        if (!url) {
            return;
        }
        AVURLAsset *a = [AVURLAsset URLAssetWithURL:url options:nil];
        AVAssetTrack *t = first_track(a, AVMediaTypeAudio);
        if (!t) {
            return;
        }
        AVMutableCompositionTrack *ca =
            [comp addMutableTrackWithMediaType:AVMediaTypeAudio preferredTrackID:kCMPersistentTrackID_Invalid];
        CMTime ad = CMTIME_COMPARE_INLINE(a.duration, >, duration) ? duration : a.duration;
        [ca insertTimeRange:CMTimeRangeMake(kCMTimeZero, ad) ofTrack:t atTime:kCMTimeZero error:nil];
    };
    addAudio(mic);
    addAudio(sysAudio);

    [[NSFileManager defaultManager] removeItemAtURL:outURL error:nil];
    AVAssetExportSession *exp = [[AVAssetExportSession alloc] initWithAsset:comp
                                                                  presetName:AVAssetExportPresetHighestQuality];
    exp.outputURL = outURL;
    exp.outputFileType = AVFileTypeMPEG4;
    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    __block int ok = 0;
    __block NSError *exportErr = nil;
    [exp exportAsynchronouslyWithCompletionHandler:^{
        ok = exp.status == AVAssetExportSessionStatusCompleted;
        exportErr = exp.error;
        dispatch_semaphore_signal(sem);
    }];
    dispatch_semaphore_wait(sem, DISPATCH_TIME_FOREVER);
    if (!ok && err) {
        *err = exportErr;
    }
    return ok ? 0 : -1;
}

AsterismMacRecorder *asterism_macos_recorder_start(
    const char *output_path,
    int width,
    int height,
    int fps,
    int audio_mode,
    char *err,
    int errlen
) {
    if (!output_path || width < 2 || height < 2) {
        set_err(err, errlen, @"invalid recorder arguments");
        return NULL;
    }
    width &= ~1;
    height &= ~1;
    fps = fps < 10 ? 10 : (fps > 60 ? 60 : fps);

    @autoreleasepool {
        NSError *nserr = nil;
        NSURL *outURL = [NSURL fileURLWithPath:[NSString stringWithUTF8String:output_path]];
        NSURL *videoURL = temp_url(@"mp4");
        [[NSFileManager defaultManager] removeItemAtURL:videoURL error:nil];

        AVAssetWriter *writer = [[AVAssetWriter alloc] initWithURL:videoURL fileType:AVFileTypeMPEG4 error:&nserr];
        if (!writer) {
            set_err(err, errlen, nserr.localizedDescription ?: @"AVAssetWriter init failed");
            return NULL;
        }
        NSDictionary *videoSettings = @{
            AVVideoCodecKey: AVVideoCodecTypeH264,
            AVVideoWidthKey: @(width),
            AVVideoHeightKey: @(height),
            AVVideoCompressionPropertiesKey: @{
                AVVideoAverageBitRateKey: @(width * height * 4),
                AVVideoProfileLevelKey: AVVideoProfileLevelH264HighAutoLevel,
            },
        };
        AVAssetWriterInput *videoIn =
            [[AVAssetWriterInput alloc] initWithMediaType:AVMediaTypeVideo outputSettings:videoSettings];
        videoIn.expectsMediaDataInRealTime = YES;
        NSDictionary *srcAttrs = @{
            (id)kCVPixelBufferPixelFormatTypeKey: @(kCVPixelFormatType_32BGRA),
            (id)kCVPixelBufferWidthKey: @(width),
            (id)kCVPixelBufferHeightKey: @(height),
            (id)kCVPixelBufferIOSurfacePropertiesKey: @{},
        };
        AVAssetWriterInputPixelBufferAdaptor *adaptor =
            [[AVAssetWriterInputPixelBufferAdaptor alloc] initWithAssetWriterInput:videoIn
                                                       sourcePixelBufferAttributes:srcAttrs];
        if (![writer canAddInput:videoIn]) {
            set_err(err, errlen, @"cannot add video input");
            return NULL;
        }
        [writer addInput:videoIn];
        if (![writer startWriting]) {
            set_err(err, errlen, writer.error.localizedDescription ?: @"startWriting failed");
            return NULL;
        }
        [writer startSessionAtSourceTime:kCMTimeZero];

        AsterismMacRecorder *rec = calloc(1, sizeof(AsterismMacRecorder));
        rec->writer = writer;
        rec->videoIn = videoIn;
        rec->adaptor = adaptor;
        rec->videoURL = videoURL;
        rec->outputURL = outURL;
        rec->width = width;
        rec->height = height;
        rec->fps = fps;
        rec->audioMode = audio_mode;
        rec->started = 1;
        CFRetain((__bridge CFTypeRef)writer);
        CFRetain((__bridge CFTypeRef)videoIn);
        CFRetain((__bridge CFTypeRef)adaptor);
        CFRetain((__bridge CFTypeRef)videoURL);
        CFRetain((__bridge CFTypeRef)outURL);

        if (audio_mode == ASTERISM_AUDIO_MIC || audio_mode == ASTERISM_AUDIO_BOTH) {
            if (!asterism_macos_mic_access_ok()) {
                asterism_macos_request_mic_access();
            }
            NSURL *micURL = temp_url(@"m4a");
            rec->micURL = micURL;
            CFRetain((__bridge CFTypeRef)micURL);
            NSDictionary *micSettings = @{
                AVFormatIDKey: @(kAudioFormatMPEG4AAC),
                AVSampleRateKey: @48000,
                AVNumberOfChannelsKey: @1,
                AVEncoderBitRateKey: @128000,
            };
            rec->mic = [[AVAudioRecorder alloc] initWithURL:micURL settings:micSettings error:&nserr];
            if (rec->mic) {
                CFRetain((__bridge CFTypeRef)rec->mic);
                [rec->mic record];
            }
        }

        if ((audio_mode == ASTERISM_AUDIO_SYSTEM || audio_mode == ASTERISM_AUDIO_BOTH)) {
          if (@available(macOS 13.0, *)) {
            dispatch_semaphore_t sem = dispatch_semaphore_create(0);
            __block SCShareableContent *content = nil;
            __block NSError *scErr = nil;
            [SCShareableContent
                getShareableContentExcludingDesktopWindows:NO
                                       onScreenWindowsOnly:YES
                                         completionHandler:^(SCShareableContent *c, NSError *e) {
                                             content = c;
                                             scErr = e;
                                             dispatch_semaphore_signal(sem);
                                         }];
            dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 3 * NSEC_PER_SEC));
            SCDisplay *display = content.displays.firstObject;
            if (display) {
                NSURL *sysURL = temp_url(@"caf");
                rec->sysURL = sysURL;
                CFRetain((__bridge CFTypeRef)sysURL);
                AudioStreamBasicDescription asbd = {0};
                asbd.mSampleRate = 48000;
                asbd.mFormatID = kAudioFormatLinearPCM;
                asbd.mFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked;
                asbd.mBitsPerChannel = 32;
                asbd.mChannelsPerFrame = 2;
                asbd.mFramesPerPacket = 1;
                asbd.mBytesPerFrame = 8;
                asbd.mBytesPerPacket = 8;
                ExtAudioFileRef ext = NULL;
                if (ExtAudioFileCreateWithURL(
                        (__bridge CFURLRef)sysURL, kAudioFileCAFType, &asbd, NULL, kAudioFileFlags_EraseFile, &ext
                    ) == noErr) {
                    AsterismAudioSink *sink = [AsterismAudioSink new];
                    sink.extFile = ext;
                    sink.running = YES;
                    rec->sysSink = sink;
                    CFRetain((__bridge CFTypeRef)sink);
                    SCContentFilter *filter =
                        [[SCContentFilter alloc] initWithDisplay:display excludingWindows:@[]];
                    SCStreamConfiguration *cfg = [SCStreamConfiguration new];
                    cfg.capturesAudio = YES;
                    cfg.excludesCurrentProcessAudio = YES;
                    cfg.width = 8;
                    cfg.height = 8;
                    cfg.minimumFrameInterval = CMTimeMake(1, 1);
                    SCStream *stream = [[SCStream alloc] initWithFilter:filter configuration:cfg delegate:sink];
                    NSError *addErr = nil;
                    [stream addStreamOutput:sink
                                       type:SCStreamOutputTypeAudio
                         sampleHandlerQueue:dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0)
                                      error:&addErr];
                    if (!addErr) {
                        rec->sysStream = stream;
                        CFRetain((__bridge CFTypeRef)stream);
                        [stream startCaptureWithCompletionHandler:^(NSError *e) {
                            (void)e;
                        }];
                    }
                }
            } else if (scErr) {
                set_err(err, errlen, scErr.localizedDescription);
            }
          }
        }
        return rec;
    }
}

int asterism_macos_recorder_push_bgra(
    AsterismMacRecorder *rec,
    const uint8_t *bgra,
    int width,
    int height,
    int64_t pts_us,
    char *err,
    int errlen
) {
    if (!rec || !bgra) {
        set_err(err, errlen, @"null recorder");
        return -1;
    }
    @autoreleasepool {
        if (!rec->videoIn.readyForMoreMediaData) {
            for (int i = 0; i < 50 && !rec->videoIn.readyForMoreMediaData; i++) {
                [NSThread sleepForTimeInterval:0.002];
            }
            if (!rec->videoIn.readyForMoreMediaData) {
                return 0;
            }
        }
        CVPixelBufferRef pb = NULL;
        NSDictionary *attrs = @{(id)kCVPixelBufferIOSurfacePropertiesKey: @{}};
        CVReturn cr = CVPixelBufferCreate(
            kCFAllocatorDefault, rec->width, rec->height, kCVPixelFormatType_32BGRA,
            (__bridge CFDictionaryRef)attrs, &pb
        );
        if (cr != kCVReturnSuccess || !pb) {
            set_err(err, errlen, @"CVPixelBufferCreate failed");
            return -1;
        }
        CVPixelBufferLockBaseAddress(pb, 0);
        uint8_t *dst = CVPixelBufferGetBaseAddress(pb);
        size_t stride = CVPixelBufferGetBytesPerRow(pb);
        int srcStride = width * 4;
        int copyH = height < rec->height ? height : rec->height;
        int copyW = width < rec->width ? width : rec->width;
        for (int y = 0; y < copyH; y++) {
            memcpy(dst + (size_t)y * stride, bgra + (size_t)y * srcStride, (size_t)copyW * 4);
        }
        CVPixelBufferUnlockBaseAddress(pb, 0);
        CMTime pts = CMTimeMake(pts_us, 1000000);
        BOOL ok = [rec->adaptor appendPixelBuffer:pb withPresentationTime:pts];
        CVPixelBufferRelease(pb);
        if (!ok) {
            set_err(err, errlen, rec->writer.error.localizedDescription ?: @"appendPixelBuffer failed");
            return -1;
        }
        return 0;
    }
}

int asterism_macos_recorder_finish(AsterismMacRecorder *rec, char *err, int errlen) {
    if (!rec) {
        return -1;
    }
    @autoreleasepool {
        [rec->mic stop];
        if (rec->sysStream) {
            dispatch_semaphore_t sem = dispatch_semaphore_create(0);
            [rec->sysStream stopCaptureWithCompletionHandler:^(NSError *e) {
                (void)e;
                dispatch_semaphore_signal(sem);
            }];
            dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 2 * NSEC_PER_SEC));
        }
        if (rec->sysSink) {
            rec->sysSink.running = NO;
            if (rec->sysSink.extFile) {
                ExtAudioFileDispose(rec->sysSink.extFile);
                rec->sysSink.extFile = NULL;
            }
        }
        [rec->videoIn markAsFinished];
        dispatch_semaphore_t sem = dispatch_semaphore_create(0);
        [rec->writer finishWritingWithCompletionHandler:^{ dispatch_semaphore_signal(sem); }];
        dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 8 * NSEC_PER_SEC));

        NSError *nserr = nil;
        BOOL haveExtra = rec->micURL || rec->sysURL;
        if (haveExtra) {
            if (mux_tracks(rec->videoURL, rec->micURL, rec->sysURL, rec->outputURL, &nserr) != 0) {
                [[NSFileManager defaultManager] copyItemAtURL:rec->videoURL toURL:rec->outputURL error:nil];
            }
        } else {
            [[NSFileManager defaultManager] removeItemAtURL:rec->outputURL error:nil];
            [[NSFileManager defaultManager] copyItemAtURL:rec->videoURL toURL:rec->outputURL error:&nserr];
        }
        if (nserr && err) {
            set_err(err, errlen, nserr.localizedDescription);
        }

        if (rec->writer) CFRelease((__bridge CFTypeRef)rec->writer);
        if (rec->videoIn) CFRelease((__bridge CFTypeRef)rec->videoIn);
        if (rec->adaptor) CFRelease((__bridge CFTypeRef)rec->adaptor);
        if (rec->mic) CFRelease((__bridge CFTypeRef)rec->mic);
        if (rec->sysStream) CFRelease((__bridge CFTypeRef)rec->sysStream);
        if (rec->sysSink) CFRelease((__bridge CFTypeRef)rec->sysSink);
        if (rec->videoURL) CFRelease((__bridge CFTypeRef)rec->videoURL);
        if (rec->micURL) CFRelease((__bridge CFTypeRef)rec->micURL);
        if (rec->sysURL) CFRelease((__bridge CFTypeRef)rec->sysURL);
        if (rec->outputURL) CFRelease((__bridge CFTypeRef)rec->outputURL);
        free(rec);
        return nserr ? -1 : 0;
    }
}
