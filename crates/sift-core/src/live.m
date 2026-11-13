// Live Photo writer for Apple platforms: an H.264 QuickTime movie and a JPEG
// that carry the same content identifier, which is what makes Photos treat
// the two files as one Live Photo. Called from `live.rs`.

#import <AVFoundation/AVFoundation.h>
#import <CoreMedia/CoreMedia.h>
#import <CoreVideo/CoreVideo.h>
#import <ImageIO/ImageIO.h>

static NSString *const kContentIdentifier = @"com.apple.quicktime.content.identifier";
static NSString *const kStillImageTime = @"com.apple.quicktime.still-image-time";
static const int32_t kTimescale = 600;

@interface SiftLive : NSObject
@property(nonatomic, strong) AVAssetWriter *writer;
@property(nonatomic, strong) AVAssetWriterInput *video;
@property(nonatomic, strong) AVAssetWriterInputPixelBufferAdaptor *pixels;
@property(nonatomic, strong) AVAssetWriterInputMetadataAdaptor *marks;
@property(nonatomic) int width;
@property(nonatomic) int height;
@end

@implementation SiftLive
@end

static void report(char *err, int cap, NSString *message) {
    if (err && cap > 0) {
        strlcpy(err, message.UTF8String ?: "unknown error", (size_t)cap);
    }
}

static AVMetadataItem *item(NSString *key, id value, NSString *type) {
    AVMutableMetadataItem *it = [AVMutableMetadataItem metadataItem];
    it.keySpace = AVMetadataKeySpaceQuickTimeMetadata;
    it.key = key;
    it.value = value;
    it.dataType = type;
    return it;
}

static BOOL wait_ready(AVAssetWriterInput *input) {
    for (int i = 0; i < 5000 && !input.readyForMoreMediaData; i++) {
        usleep(2000);
    }
    return input.readyForMoreMediaData;
}

void *sift_live_begin(const char *path, int width, int height, const char *identifier, char *err, int cap) {
    @autoreleasepool {
        NSURL *url = [NSURL fileURLWithPath:@(path)];
        [NSFileManager.defaultManager removeItemAtURL:url error:nil];
        NSError *error = nil;
        AVAssetWriter *writer = [AVAssetWriter assetWriterWithURL:url fileType:AVFileTypeQuickTimeMovie error:&error];
        if (!writer) {
            report(err, cap, error.localizedDescription);
            return NULL;
        }
        writer.metadata = @[item(kContentIdentifier, @(identifier), (__bridge NSString *)kCMMetadataBaseDataType_UTF8)];

        NSDictionary *settings = @{
            AVVideoCodecKey: AVVideoCodecTypeH264,
            AVVideoWidthKey: @(width),
            AVVideoHeightKey: @(height),
            AVVideoCompressionPropertiesKey: @{
                AVVideoAverageBitRateKey: @((double)width * height * 9),
                AVVideoProfileLevelKey: AVVideoProfileLevelH264HighAutoLevel,
                AVVideoAllowFrameReorderingKey: @NO,
            },
        };
        AVAssetWriterInput *video = [AVAssetWriterInput assetWriterInputWithMediaType:AVMediaTypeVideo outputSettings:settings];
        video.expectsMediaDataInRealTime = NO;
        AVAssetWriterInputPixelBufferAdaptor *pixels = [AVAssetWriterInputPixelBufferAdaptor
            assetWriterInputPixelBufferAdaptorWithAssetWriterInput:video
            sourcePixelBufferAttributes:@{
                (__bridge NSString *)kCVPixelBufferPixelFormatTypeKey: @(kCVPixelFormatType_32BGRA),
                (__bridge NSString *)kCVPixelBufferWidthKey: @(width),
                (__bridge NSString *)kCVPixelBufferHeightKey: @(height),
            }];

        // The track that says which moment of the movie the still shows.
        NSDictionary *spec = @{
            (__bridge NSString *)kCMMetadataFormatDescriptionMetadataSpecificationKey_Identifier: [@"mdta/" stringByAppendingString:kStillImageTime],
            (__bridge NSString *)kCMMetadataFormatDescriptionMetadataSpecificationKey_DataType: (__bridge NSString *)kCMMetadataBaseDataType_SInt8,
        };
        CMFormatDescriptionRef format = NULL;
        CMMetadataFormatDescriptionCreateWithMetadataSpecifications(kCFAllocatorDefault, kCMMetadataFormatType_Boxed, (__bridge CFArrayRef) @[spec], &format);
        AVAssetWriterInput *track = [AVAssetWriterInput assetWriterInputWithMediaType:AVMediaTypeMetadata outputSettings:nil sourceFormatHint:format];
        if (format) {
            CFRelease(format);
        }
        AVAssetWriterInputMetadataAdaptor *marks = [AVAssetWriterInputMetadataAdaptor assetWriterInputMetadataAdaptorWithAssetWriterInput:track];

        if (![writer canAddInput:video] || ![writer canAddInput:track]) {
            report(err, cap, @"the system cannot encode this movie");
            return NULL;
        }
        [writer addInput:video];
        [writer addInput:track];
        if (![writer startWriting]) {
            report(err, cap, writer.error.localizedDescription);
            return NULL;
        }
        [writer startSessionAtSourceTime:kCMTimeZero];

        SiftLive *live = [SiftLive new];
        live.writer = writer;
        live.video = video;
        live.pixels = pixels;
        live.marks = marks;
        live.width = width;
        live.height = height;
        return (void *)CFBridgingRetain(live);
    }
}

/// Appends one frame of tightly packed BGRA rows, shown from `seconds` on.
int sift_live_frame(void *handle, const uint8_t *bgra, double seconds, char *err, int cap) {
    @autoreleasepool {
        SiftLive *live = (__bridge SiftLive *)handle;
        if (!wait_ready(live.video)) {
            report(err, cap, live.writer.error.localizedDescription ?: @"the encoder stopped taking frames");
            return 0;
        }
        CVPixelBufferRef buffer = NULL;
        if (CVPixelBufferPoolCreatePixelBuffer(kCFAllocatorDefault, live.pixels.pixelBufferPool, &buffer) != kCVReturnSuccess || !buffer) {
            report(err, cap, @"out of memory");
            return 0;
        }
        CVPixelBufferLockBaseAddress(buffer, 0);
        uint8_t *base = CVPixelBufferGetBaseAddress(buffer);
        size_t stride = CVPixelBufferGetBytesPerRow(buffer);
        size_t row = (size_t)live.width * 4;
        for (int y = 0; y < live.height; y++) {
            memcpy(base + y * stride, bgra + y * row, row);
        }
        CVPixelBufferUnlockBaseAddress(buffer, 0);
        BOOL ok = [live.pixels appendPixelBuffer:buffer withPresentationTime:CMTimeMakeWithSeconds(seconds, kTimescale)];
        CVPixelBufferRelease(buffer);
        if (!ok) {
            report(err, cap, live.writer.error.localizedDescription ?: @"cannot append frame");
        }
        return ok;
    }
}

/// Marks the still's moment, ends the movie at `duration` and releases the handle.
int sift_live_finish(void *handle, double still, double duration, char *err, int cap) {
    @autoreleasepool {
        SiftLive *live = (SiftLive *)CFBridgingRelease(handle);
        AVAssetWriter *writer = live.writer;
        if (wait_ready(live.marks.assetWriterInput)) {
            AVMetadataItem *mark = item(kStillImageTime, @0, (__bridge NSString *)kCMMetadataBaseDataType_SInt8);
            CMTimeRange range = CMTimeRangeMake(CMTimeMakeWithSeconds(still, kTimescale), CMTimeMake(kTimescale / 30, kTimescale));
            [live.marks appendTimedMetadataGroup:[[AVTimedMetadataGroup alloc] initWithItems:@[mark] timeRange:range]];
        }
        [live.marks.assetWriterInput markAsFinished];
        [live.video markAsFinished];
        [writer endSessionAtSourceTime:CMTimeMakeWithSeconds(duration, kTimescale)];
        dispatch_semaphore_t done = dispatch_semaphore_create(0);
        [writer finishWritingWithCompletionHandler:^{
            dispatch_semaphore_signal(done);
        }];
        dispatch_semaphore_wait(done, DISPATCH_TIME_FOREVER);
        if (writer.status != AVAssetWriterStatusCompleted) {
            report(err, cap, writer.error.localizedDescription ?: @"cannot finish the movie");
            return 0;
        }
        return 1;
    }
}

/// Drops a movie that will not be finished.
void sift_live_abort(void *handle) {
    @autoreleasepool {
        SiftLive *live = (SiftLive *)CFBridgingRelease(handle);
        [live.writer cancelWriting];
    }
}

/// Writes `jpeg` to `path` with the content identifier in its Apple maker
/// note; everything else the picture records (EXIF, orientation) is kept.
int sift_live_still(const uint8_t *jpeg, long length, const char *path, const char *identifier, char *err, int cap) {
    @autoreleasepool {
        NSData *data = [NSData dataWithBytesNoCopy:(void *)jpeg length:(NSUInteger)length freeWhenDone:NO];
        CGImageSourceRef source = CGImageSourceCreateWithData((__bridge CFDataRef)data, NULL);
        if (!source || CGImageSourceGetCount(source) == 0) {
            if (source) {
                CFRelease(source);
            }
            report(err, cap, @"the still cannot be read");
            return 0;
        }
        NSMutableDictionary *properties = [(__bridge_transfer NSDictionary *)CGImageSourceCopyPropertiesAtIndex(source, 0, NULL) mutableCopy] ?: [NSMutableDictionary new];
        properties[(__bridge NSString *)kCGImagePropertyMakerAppleDictionary] = @{@"17": @(identifier)};
        properties[(__bridge NSString *)kCGImageDestinationLossyCompressionQuality] = @0.95;
        NSURL *url = [NSURL fileURLWithPath:@(path)];
        CGImageDestinationRef destination = CGImageDestinationCreateWithURL((__bridge CFURLRef)url, CFSTR("public.jpeg"), 1, NULL);
        BOOL ok = NO;
        if (destination) {
            CGImageDestinationAddImageFromSource(destination, source, 0, (__bridge CFDictionaryRef)properties);
            ok = CGImageDestinationFinalize(destination);
            CFRelease(destination);
        }
        CFRelease(source);
        if (!ok) {
            report(err, cap, @"cannot write the still");
        }
        return ok;
    }
}
