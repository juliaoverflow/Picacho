// Picacho's iOS host app. The app is the Rust library (src/ios.rs, linked
// as libpicacho.a); this file only gives it a scene, a window and frames,
// and hands the system's events to the Swift beside it (notifications,
// background refresh, shares).
//
// gpui-mobile runs embedded: it makes the view controller, and this scene
// puts it in its UIWindow. A CADisplayLink asks GPUI for frames and pauses
// while GPUI has nothing to draw; GPUI's frame waker resumes it.

#import <CoreSpotlight/CoreSpotlight.h>
#import <QuartzCore/QuartzCore.h>
#import <UIKit/UIKit.h>

extern bool picacho_ios_start(void);
extern void gpui_ios_set_embedded(void);
extern void *gpui_ios_get_window(void);
extern void *gpui_ios_view_controller(void *window);
extern void gpui_ios_layout_view(void *window);
extern bool gpui_ios_request_frame(void *window);
extern void gpui_ios_set_frame_waker(void *window, void (*waker)(void *), void *context);
extern void gpui_ios_will_enter_foreground(void *app);
extern void gpui_ios_did_become_active(void *app);
extern void gpui_ios_will_resign_active(void *app);
extern void gpui_ios_did_enter_background(void *app);
extern void gpui_ios_will_terminate(void *app);
extern void gpui_ios_handle_open_url(void *url);

// The Swift side (packaging/ios/*.swift).
extern void picacho_ios_app_launched(void);
extern void picacho_ios_scene_started(void);
extern void picacho_ios_scene_backgrounded(void);
extern void picacho_ios_collect_shares(void);
extern bool picacho_ios_privacy_cover(void);
// The Rust library (src/ios.rs): something for the window to open.
extern void picacho_ios_launch(const uint8_t *request, size_t len);

// Opens `url`: a share the Share extension handed over
// (<scheme>://share/<id>) is taken here; the rest go to GPUI (sign-in
// redirects, widgets' links).
static void openURL(NSURL *url) {
    NSString *scheme = url.scheme.lowercaseString;
    BOOL ours = [scheme isEqualToString:@"picacho"] ||
                [scheme isEqualToString:NSBundle.mainBundle.bundleIdentifier.lowercaseString];
    if (ours && [url.host isEqualToString:@"share"]) {
        picacho_ios_collect_shares();
        return;
    }
    gpui_ios_handle_open_url((__bridge void *)url.absoluteString);
}

// A Spotlight result chosen (src/manage/system_search/spotlight.rs): its
// identifier is the message's picacho:// link, which the window opens (and
// Rust checks, as any link). Other activities aren't Picacho's.
static void openActivity(NSUserActivity *activity) {
    if (![activity.activityType isEqualToString:CSSearchableItemActionType]) {
        return;
    }
    id link = activity.userInfo[CSSearchableItemActivityIdentifier];
    if (![link isKindOfClass:[NSString class]]) {
        return;
    }
    NSData *bytes = [(NSString *)link dataUsingEncoding:NSUTF8StringEncoding];
    picacho_ios_launch(bytes.bytes, bytes.length);
}

@interface PicachoScene : UIResponder <UIWindowSceneDelegate>
@property(nonatomic, strong) UIWindow *window;
@property(nonatomic, strong) CADisplayLink *link;
@property(nonatomic) void *gpuiWindow;
// Over the window while Picacho is away with the app lock on.
@property(nonatomic, strong) UIView *privacyCover;
- (void)tick:(CADisplayLink *)link;
@end

// The scene showing GPUI's window, if any. GPUI keeps one window for the
// process; a scene iOS disconnects (and later connects again) hands it on.
// Weak: the waker below is given no reference of its own to keep alive.
static __weak PicachoScene *current;

// GPUI wants a frame (a view changed, an animation, typed text).
static void wake(void *context) {
    (void)context;
    if ([NSThread isMainThread]) {
        current.link.paused = NO;
    } else {
        dispatch_async(dispatch_get_main_queue(), ^{ current.link.paused = NO; });
    }
}

// Forwards the display link's ticks without keeping the scene alive (the
// link retains its target; the scene retains the link).
@interface PicachoTicker : NSObject
@property(nonatomic, weak) PicachoScene *scene;
@end

@implementation PicachoScene

- (void)scene:(UIScene *)scene
    willConnectToSession:(UISceneSession *)session
                 options:(UISceneConnectionOptions *)options {
    static BOOL started = NO;
    if (started) {
        // A scene connected again (iOS let the earlier one go): it shows the
        // same window.
        [self attachToScene:(UIWindowScene *)scene];
        for (NSUserActivity *activity in options.userActivities) {
            openActivity(activity);
        }
        return;
    }
    started = YES;
    gpui_ios_set_embedded();
    // The mail cache (mail.sqlite3, readable mail) stays out of iCloud and
    // computer backups, as Android's does: it downloads again, and backups
    // would otherwise keep a copy of the mail outside this device.
    NSURL *support = [[NSFileManager.defaultManager URLsForDirectory:NSApplicationSupportDirectory
                                                           inDomains:NSUserDomainMask] firstObject];
    NSURL *data = [support URLByAppendingPathComponent:@"picacho" isDirectory:YES];
    [NSFileManager.defaultManager createDirectoryAtURL:data
                           withIntermediateDirectories:YES
                                            attributes:nil
                                                 error:nil];
    NSError *excluded = nil;
    if (![data setResourceValue:@YES forKey:NSURLIsExcludedFromBackupKey error:&excluded]) {
        NSLog(@"Couldn't keep the mail cache out of backups: %@", excluded);
    }
    if (!picacho_ios_start()) {
        NSLog(@"Picacho did not start; see the log above.");
        return;
    }
    gpui_ios_set_frame_waker(gpui_ios_get_window(), wake, NULL);
    [self attachToScene:(UIWindowScene *)scene];
    picacho_ios_scene_started();

    for (UIOpenURLContext *context in options.URLContexts) {
        openURL(context.URL);
    }
    // Started by choosing a Spotlight result.
    for (NSUserActivity *activity in options.userActivities) {
        openActivity(activity);
    }
}

// Shows GPUI's window in `scene`, with a display link of its own.
- (void)attachToScene:(UIWindowScene *)scene {
    self.gpuiWindow = gpui_ios_get_window();
    UIViewController *controller =
        (__bridge UIViewController *)gpui_ios_view_controller(self.gpuiWindow);
    self.window = [[UIWindow alloc] initWithWindowScene:scene];
    self.window.rootViewController = controller;
    [self.window makeKeyAndVisible];
    gpui_ios_layout_view(self.gpuiWindow);

    PicachoTicker *ticker = [PicachoTicker new];
    ticker.scene = self;
    self.link = [CADisplayLink displayLinkWithTarget:ticker selector:@selector(tick:)];
    // Up to the screen's rate (120 Hz on ProMotion); iOS lowers it when idle.
    self.link.preferredFrameRateRange = CAFrameRateRangeMake(30, 120, 120);
    [self.link addToRunLoop:[NSRunLoop mainRunLoop] forMode:NSRunLoopCommonModes];
    current = self;
}

// iOS let the scene go (it may connect a new one later): its display link
// stops, and nothing calls into it any more.
- (void)sceneDidDisconnect:(UIScene *)scene {
    [self.link invalidate];
    self.link = nil;
    if (current == self) {
        current = nil;
    }
    self.window.rootViewController = nil;
    self.window = nil;
}

- (void)tick:(CADisplayLink *)link {
    if (!gpui_ios_request_frame(self.gpuiWindow)) {
        link.paused = YES;
    }
}

- (void)scene:(UIScene *)scene openURLContexts:(NSSet<UIOpenURLContext *> *)contexts {
    for (UIOpenURLContext *context in contexts) {
        openURL(context.URL);
    }
}

// A Spotlight result chosen while Picacho ran.
- (void)scene:(UIScene *)scene continueUserActivity:(NSUserActivity *)userActivity {
    openActivity(userActivity);
}

- (void)sceneWillEnterForeground:(UIScene *)scene {
    gpui_ios_will_enter_foreground(NULL);
    self.link.paused = NO;
}

- (void)sceneDidBecomeActive:(UIScene *)scene {
    gpui_ios_did_become_active(NULL);
    self.link.paused = NO;
    [self.privacyCover removeFromSuperview];
    self.privacyCover = nil;
    // A share the extension couldn't open Picacho for.
    picacho_ios_collect_shares();
}

- (void)sceneWillResignActive:(UIScene *)scene {
    gpui_ios_will_resign_active(NULL);
    // With the app lock on, a cover of UIKit's own over the window before
    // iOS pictures it for the app switcher: GPUI's privacy cover needs a
    // frame drawn, and the display link stops as Picacho goes to the
    // background, so the picture could show the mail.
    if (picacho_ios_privacy_cover() && self.privacyCover == nil && self.window != nil) {
        UIVisualEffectView *cover = [[UIVisualEffectView alloc]
            initWithEffect:[UIBlurEffect effectWithStyle:UIBlurEffectStyleSystemMaterial]];
        cover.frame = self.window.bounds;
        cover.autoresizingMask = UIViewAutoresizingFlexibleWidth | UIViewAutoresizingFlexibleHeight;
        [self.window addSubview:cover];
        self.privacyCover = cover;
    }
}

- (void)sceneDidEnterBackground:(UIScene *)scene {
    gpui_ios_did_enter_background(NULL);
    self.link.paused = YES;
    picacho_ios_scene_backgrounded();
}

@end

@implementation PicachoTicker
- (void)tick:(CADisplayLink *)link {
    PicachoScene *scene = self.scene;
    if (scene == nil) {
        [link invalidate];
        return;
    }
    [scene tick:link];
}
@end

@interface PicachoApp : UIResponder <UIApplicationDelegate>
@end

@implementation PicachoApp

// Before launching ends: the notification center's delegate (a tap may
// have launched Picacho) and the background refresh's handler. iOS may
// launch Picacho in the background for either, without a scene.
- (BOOL)application:(UIApplication *)application
    didFinishLaunchingWithOptions:(NSDictionary<UIApplicationLaunchOptionsKey, id> *)options {
    picacho_ios_app_launched();
    return YES;
}

- (UISceneConfiguration *)application:(UIApplication *)application
    configurationForConnectingSceneSession:(UISceneSession *)session
                                   options:(UISceneConnectionOptions *)options {
    UISceneConfiguration *config = [[UISceneConfiguration alloc] initWithName:@"Picacho"
                                                                  sessionRole:session.role];
    config.delegateClass = [PicachoScene class];
    return config;
}

- (void)applicationWillTerminate:(UIApplication *)application {
    gpui_ios_will_terminate(NULL);
}

@end

int main(int argc, char *argv[]) {
    @autoreleasepool {
        return UIApplicationMain(argc, argv, nil, NSStringFromClass([PicachoApp class]));
    }
}
