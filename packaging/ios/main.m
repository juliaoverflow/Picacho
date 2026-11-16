// Picacho's iOS host app. The app is the Rust library (src/ios.rs, linked
// as libpicacho.a); this file only gives it a scene, a window and frames.
//
// gpui-mobile runs embedded: it makes the view controller, and this scene
// puts it in its UIWindow. A CADisplayLink asks GPUI for frames and pauses
// while GPUI has nothing to draw; GPUI's frame waker resumes it.

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

@interface PicachoScene : UIResponder <UIWindowSceneDelegate>
@property(nonatomic, strong) UIWindow *window;
@property(nonatomic, strong) CADisplayLink *link;
@property(nonatomic) void *gpuiWindow;
@end

// GPUI wants a frame (a view changed, an animation, typed text).
static void wake(void *context) {
    PicachoScene *scene = (__bridge PicachoScene *)context;
    if ([NSThread isMainThread]) {
        scene.link.paused = NO;
    } else {
        dispatch_async(dispatch_get_main_queue(), ^{ scene.link.paused = NO; });
    }
}

@implementation PicachoScene

- (void)scene:(UIScene *)scene
    willConnectToSession:(UISceneSession *)session
                 options:(UISceneConnectionOptions *)options {
    static BOOL started = NO;
    if (started) {
        return; // Picacho shows one window.
    }
    started = YES;
    gpui_ios_set_embedded();
    if (!picacho_ios_start()) {
        NSLog(@"Picacho did not start; see the log above.");
        return;
    }
    self.gpuiWindow = gpui_ios_get_window();
    UIViewController *controller =
        (__bridge UIViewController *)gpui_ios_view_controller(self.gpuiWindow);

    self.window = [[UIWindow alloc] initWithWindowScene:(UIWindowScene *)scene];
    self.window.rootViewController = controller;
    [self.window makeKeyAndVisible];
    gpui_ios_layout_view(self.gpuiWindow);

    self.link = [CADisplayLink displayLinkWithTarget:self selector:@selector(tick:)];
    // Up to the screen's rate (120 Hz on ProMotion); iOS lowers it when idle.
    self.link.preferredFrameRateRange = CAFrameRateRangeMake(30, 120, 120);
    [self.link addToRunLoop:[NSRunLoop mainRunLoop] forMode:NSRunLoopCommonModes];
    gpui_ios_set_frame_waker(self.gpuiWindow, wake, (__bridge void *)self);

    for (UIOpenURLContext *context in options.URLContexts) {
        gpui_ios_handle_open_url((__bridge void *)context.URL.absoluteString);
    }
}

- (void)tick:(CADisplayLink *)link {
    if (!gpui_ios_request_frame(self.gpuiWindow)) {
        link.paused = YES;
    }
}

- (void)scene:(UIScene *)scene openURLContexts:(NSSet<UIOpenURLContext *> *)contexts {
    for (UIOpenURLContext *context in contexts) {
        gpui_ios_handle_open_url((__bridge void *)context.URL.absoluteString);
    }
}

- (void)sceneWillEnterForeground:(UIScene *)scene {
    gpui_ios_will_enter_foreground(NULL);
    self.link.paused = NO;
}

- (void)sceneDidBecomeActive:(UIScene *)scene {
    gpui_ios_did_become_active(NULL);
    self.link.paused = NO;
}

- (void)sceneWillResignActive:(UIScene *)scene {
    gpui_ios_will_resign_active(NULL);
}

- (void)sceneDidEnterBackground:(UIScene *)scene {
    gpui_ios_did_enter_background(NULL);
    self.link.paused = YES;
}

@end

@interface PicachoApp : UIResponder <UIApplicationDelegate>
@end

@implementation PicachoApp

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
