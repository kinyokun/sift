// Saving to the system photo library on iOS (Photos framework). A still and a
// movie that share a content identifier are added as one asset, which is what
// makes it a Live Photo there. Called from `photos.rs`.

#import <Photos/Photos.h>

static void report(char *err, int cap, NSString *message) {
    if (err && cap > 0) {
        strlcpy(err, message.UTF8String ?: "unknown error", (size_t)cap);
    }
}

/// Adds `still` (and `movie` as its Live Photo video, when not NULL) to the
/// photo library as one asset. Returns 1 when saved, -1 when the user has not
/// allowed adding photos, 0 on any other failure. Must not be called on the
/// main thread: it waits for the system, which may first ask the user.
int sift_photos_save(const char *still, const char *movie, char *err, int cap) {
    @autoreleasepool {
        dispatch_semaphore_t asked = dispatch_semaphore_create(0);
        __block PHAuthorizationStatus status = PHAuthorizationStatusNotDetermined;
        [PHPhotoLibrary requestAuthorizationForAccessLevel:PHAccessLevelAddOnly
                                                   handler:^(PHAuthorizationStatus answer) {
                                                       status = answer;
                                                       dispatch_semaphore_signal(asked);
                                                   }];
        dispatch_semaphore_wait(asked, DISPATCH_TIME_FOREVER);
        if (status != PHAuthorizationStatusAuthorized && status != PHAuthorizationStatusLimited) {
            report(err, cap, @"not allowed to add to the photo library");
            return -1;
        }

        NSURL *stillUrl = [NSURL fileURLWithPath:@(still)];
        NSURL *movieUrl = movie ? [NSURL fileURLWithPath:@(movie)] : nil;
        dispatch_semaphore_t done = dispatch_semaphore_create(0);
        __block BOOL ok = NO;
        __block NSString *message = nil;
        [PHPhotoLibrary.sharedPhotoLibrary
            performChanges:^{
                PHAssetCreationRequest *request = [PHAssetCreationRequest creationRequestForAsset];
                PHAssetResourceCreationOptions *photo = [PHAssetResourceCreationOptions new];
                photo.originalFilename = stillUrl.lastPathComponent;
                [request addResourceWithType:PHAssetResourceTypePhoto fileURL:stillUrl options:photo];
                if (movieUrl) {
                    PHAssetResourceCreationOptions *video = [PHAssetResourceCreationOptions new];
                    video.originalFilename = movieUrl.lastPathComponent;
                    [request addResourceWithType:PHAssetResourceTypePairedVideo fileURL:movieUrl options:video];
                }
            }
            completionHandler:^(BOOL success, NSError *error) {
                ok = success;
                message = error.localizedDescription;
                dispatch_semaphore_signal(done);
            }];
        dispatch_semaphore_wait(done, DISPATCH_TIME_FOREVER);
        if (!ok) {
            report(err, cap, message ?: @"the photo library did not take the file");
        }
        return ok ? 1 : 0;
    }
}
