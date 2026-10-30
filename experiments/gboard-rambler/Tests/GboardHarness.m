// Original-code-free simulator contract test for the injected native client.
// These stubs model the inspected selectors, not Google's implementations.
#import <UIKit/UIKit.h>
#import <dlfcn.h>
#import <objc/message.h>
#import <QuartzCore/QuartzCore.h>

static NSString *(*DiagnosticReport)(void);
static NSString *(*LastDictation)(void);
static void (*RecordDiagnostic)(NSString *, NSDictionary *);
static void (*PublishCaptureState)(BOOL);
static NSUserDefaults *VoicePreferences(void);

// Export symbol names used by the inspected app, with our own fixture values.
NSString *const kGKBVoiceInputAvailableKey = @"HarnessBackgroundAvailable";
NSString *const kGKBVoiceInputAppFallbackKey = @"HarnessBackgroundFallback";
NSString *const kGKBVoiceInputErrorString = @"HarnessVoiceError";
static NSString *const HarnessBackgroundResult = @"HarnessBackgroundResult";
static NSString *const HarnessAudioLevel = @"HarnessAudioLevel";

@interface GKBPreferences : NSObject
+ (BOOL)boolPreferenceForKey:(NSString *)key;
+ (void)setObjectPreferenceWithValue:(id)value forKey:(NSString *)key;
+ (void)setDoublePreferenceWithValue:(double)value forKey:(NSString *)key;
@end
@implementation GKBPreferences
+ (BOOL)boolPreferenceForKey:(NSString *)key { return [VoicePreferences() boolForKey:key]; }
+ (void)setObjectPreferenceWithValue:(id)value forKey:(NSString *)key { [VoicePreferences() setObject:value forKey:key]; }
+ (void)setDoublePreferenceWithValue:(double)value forKey:(NSString *)key { [VoicePreferences() setDouble:value forKey:key]; }
@end

@interface HarnessApplication : UIApplication
@property(nonatomic, strong) id returnAction;
@property(nonatomic, strong) NSURL *openedVoiceURL;
@end
@implementation HarnessApplication
- (id)_systemNavigationAction { return self.returnAction; }
- (void)gkb_openURL:(NSURL *)url { self.openedVoiceURL = url; }
@end

@interface HarnessNavigationAction : NSObject
@property(nonatomic, strong) NSArray *destinations;
@end
@implementation HarnessNavigationAction
- (BOOL)sendResponseForDestination:(unsigned int)destination { return YES; }
@end

@interface GKBAppNavigation : NSObject
@property(nonatomic) BOOL accepts, hasPendingNavigation, dropsCallbacks, repeatsCallbacks;
@property(nonatomic) NSTimeInterval callbackDelay;
@property(nonatomic) NSUInteger calls;
- (BOOL)navigateBackToPreviousAppWithStartBlock:(void (^)(void))start cleanupBlock:(void (^)(void))cleanup;
@end
@implementation GKBAppNavigation
- (instancetype)init { if ((self = [super init])) self.accepts = YES; return self; }
- (BOOL)navigateBackToPreviousAppWithStartBlock:(void (^)(void))start cleanupBlock:(void (^)(void))cleanup {
    self.calls++;
    if (!self.accepts) return NO;
    if (self.dropsCallbacks) return YES;
    void (^deliver)(void) = ^{
        if (start) start();
        if (cleanup) cleanup();
        if (self.repeatsCallbacks) { if (start) start(); if (cleanup) cleanup(); }
    };
    if (self.callbackDelay) dispatch_after(dispatch_time(DISPATCH_TIME_NOW, self.callbackDelay * NSEC_PER_SEC), dispatch_get_main_queue(), deliver);
    else deliver();
    return YES;
}
@end

@interface GKBBoolFeatureFlag : NSObject
@property(nonatomic) BOOL value;
- (NSString *)marker;
@end
@implementation GKBBoolFeatureFlag
- (NSString *)marker { return @"original flag"; }
@end
@interface GKBFeatureFlagManager : NSObject
@property(nonatomic, strong) GKBBoolFeatureFlag *voiceFlag, *unrelatedFlag;
- (id)enableInKeyboardVoiceInput;
@end
@implementation GKBFeatureFlagManager
- (instancetype)init {
    if ((self = [super init])) { self.voiceFlag = [GKBBoolFeatureFlag new]; self.unrelatedFlag = [GKBBoolFeatureFlag new]; }
    return self;
}
- (id)enableInKeyboardVoiceInput { return self.voiceFlag; }
@end

@interface GKBVoiceInputSpeechSessionConfig : NSObject
@property(nonatomic, readonly) NSString *spokenLanguage;
@end
@implementation GKBVoiceInputSpeechSessionConfig
- (NSString *)spokenLanguage { return @"en-US"; }
@end

@interface GSKSpeechToTextStreamingController : NSObject
@property(nonatomic, strong) NSObject *speechSessionConfig;
@property(nonatomic, copy) NSString *deliveredText;
@property(nonatomic, strong) NSError *error;
@property(nonatomic, weak) id delegate;
@property(nonatomic, strong) id speechVolumeStreamFactory;
@property(nonatomic) NSUInteger originalStarts, deliveries;
- (void)startVoiceSearch;
- (void)stopVoiceSearch;
- (void)stopSpeechRecognition;
- (void)pauseVoiceSearch;
- (BOOL)canPerformVoiceSearch;
- (BOOL)isPerformingVoiceSearch;
- (double)currentAudioLevel;
- (NSString *)recognitionResult;
- (NSString *)entireCurrentResult;
@end
@implementation GSKSpeechToTextStreamingController
- (void)startVoiceSearch { self.originalStarts++; }
- (void)stopVoiceSearch {}
- (void)stopSpeechRecognition {}
- (void)pauseVoiceSearch {}
- (BOOL)canPerformVoiceSearch { return NO; }
- (BOOL)isPerformingVoiceSearch { return NO; }
- (double)currentAudioLevel { return 0; }
- (NSString *)recognitionResult { return @"original"; }
- (NSString *)entireCurrentResult { return @"original"; }
- (void)updateTextViewWithStablePrefix:(id)prefix unstableSuffix:(id)suffix shouldAnimate:(BOOL)animate {}
- (void)textStreamingDidRecognizedWithResult:(NSString *)text {
    self.deliveredText = text; self.deliveries++;
    SEL selector = NSSelectorFromString(@"speechToTextStreamingController:didReceiveResult:");
    if (![self.delegate respondsToSelector:selector]) selector = NSSelectorFromString(@"glifVoiceSearchViewController:didReceiveResult:");
    if ([self.delegate respondsToSelector:selector])
        ((void (*)(id, SEL, id, id))objc_msgSend)(self.delegate, selector, self, text);
}
- (void)textStreamingDidFailWithError:(NSError *)error { self.error = error; }
@end

@interface GKBVoiceInputMonitor : NSObject {
@public GSKSpeechToTextStreamingController *_streamingController;
}
@property(nonatomic) NSUInteger getterCalls;
@property(nonatomic, strong) id audioSource;
@property(nonatomic) double lastAudioLevel;
@property(nonatomic, copy) NSString *deliveredText;
@property(nonatomic) NSUInteger deliveries;
- (void)tearDown;
- (void)handleKeyboardCancelChange:(id)value;
- (void)startMonitoringAudioLevelWithSource:(id)source;
- (void)updateAudioLevel;
@end
@implementation GKBVoiceInputMonitor
- (id)streamingController { self.getterCalls++; return _streamingController; }
- (void)stop { [_streamingController stopVoiceSearch]; }
- (void)tearDown { self.audioSource = nil; _streamingController = nil; }
- (void)handleKeyboardCancelChange:(id)value {
    [_streamingController stopSpeechRecognition];
    [self tearDown];
}
- (void)speechToTextStreamingController:(id)controller didReceiveResult:(NSString *)text {
    self.deliveredText = text; self.deliveries++;
    [GKBPreferences setObjectPreferenceWithValue:text forKey:HarnessBackgroundResult];
    [self tearDown];
}
- (void)startMonitoringAudioLevelWithSource:(id)source { self.audioSource = source; }
- (void)updateAudioLevel {
    self.lastAudioLevel = ((double (*)(id, SEL))objc_msgSend)(self.audioSource, @selector(currentAudioLevel));
    [GKBPreferences setDoublePreferenceWithValue:self.lastAudioLevel forKey:HarnessAudioLevel];
}
@end

@interface HarnessAudioSource : NSObject
@end
@implementation HarnessAudioSource
- (double)currentAudioLevel { return 0.9; }
@end

// Model the inspected UI contract separately from microphone/listener teardown.
// Completion notifies the input controller and removes the launcher's placeholder;
// keyboard cleanup removes its overlay. stopInput does neither of those things.
@interface GKBUIInputViewController : UIViewController
@property(nonatomic) NSUInteger cursorNotifications;
- (void)voiceInputDidFinish:(id)launcher;
- (void)voiceInputLauncher:(id)launcher showVoiceInputOverlay:(UIView *)overlay;
@end
@implementation GKBUIInputViewController
- (void)voiceInputDidFinish:(id)launcher { self.cursorNotifications++; }
- (void)voiceInputLauncher:(id)launcher showVoiceInputOverlay:(UIView *)overlay {
    overlay.translatesAutoresizingMaskIntoConstraints = NO;
    [self.view addSubview:overlay];
    [NSLayoutConstraint activateConstraints:@[
        [overlay.topAnchor constraintEqualToAnchor:self.view.topAnchor],
        [overlay.bottomAnchor constraintEqualToAnchor:self.view.bottomAnchor],
        [overlay.leadingAnchor constraintEqualToAnchor:self.view.leadingAnchor],
        [overlay.trailingAnchor constraintEqualToAnchor:self.view.trailingAnchor]]];
    [self.view layoutIfNeeded];
}
@end

@interface GKBVoiceInputLauncher : NSObject
@property(nonatomic, strong) GKBUIInputViewController *delegate;
@property(nonatomic, strong) UIView *placeholderOverlay;
@property(nonatomic) NSUInteger completions;
- (void)completionHandler:(id)result error:(NSError *)error;
@end
@implementation GKBVoiceInputLauncher
- (void)completionHandler:(id)result error:(NSError *)error {
    self.completions++;
    [self.delegate voiceInputDidFinish:self];
    [self.placeholderOverlay removeFromSuperview]; self.placeholderOverlay = nil;
}
@end

@interface GKBKeyboardVoiceInput : NSObject {
@public UIView *_overlay;
@public void (^_completion)(id, NSError *);
}
@property(nonatomic, strong) UITextView *document;
@property(nonatomic, strong) UIView *overlay;
@property(nonatomic, copy) void (^completion)(id, NSError *);
@property(nonatomic, strong) GKBVoiceInputLauncher *fixtureLauncher;
@property(nonatomic) NSUInteger finishCalls, cleanups, overlayGetterCalls;
@property(nonatomic, weak) GKBVoiceInputMonitor *monitor;
@property(nonatomic, strong) NSTimer *startupTimer;
@property(nonatomic, strong) NSDate *triggerDate;
@property(nonatomic) BOOL observing;
@property(nonatomic) NSUInteger timeouts, resultChanges, originalMicStarts;
@property(nonatomic) double lastAudioLevel;
- (void)start;
- (void)stopInput;
- (void)handleTextResultChange:(NSDictionary *)change;
- (void)handleAudioLevelChange:(NSDictionary *)change;
- (void)didReceiveValidResponse;
- (void)handleAppConfirmationChange:(id)payload;
- (void)voiceInputOverlayDidTapMicButton:(id)overlay;
- (void)updateDocumentProxyWithText:(NSString *)text;
- (void)finishWithResult:(id)result error:(NSError *)error;
@end
@implementation GKBKeyboardVoiceInput
@synthesize overlay = _overlay, completion = _completion;
- (UIView *)overlay {
    self.overlayGetterCalls++;
    if (!_overlay) _overlay = [UIView new];
    return _overlay;
}
- (void)cleanup { [_overlay removeFromSuperview]; _overlay = nil; self.cleanups++; }
- (void)finishWithResult:(id)result error:(NSError *)error {
    self.finishCalls++;
    if (_completion) _completion(result, error);
    _completion = nil;
    [self cleanup];
}
- (void)start {
    [GKBPreferences setObjectPreferenceWithValue:@"" forKey:HarnessBackgroundResult];
    [GKBPreferences setDoublePreferenceWithValue:0 forKey:HarnessAudioLevel];
    self.triggerDate = NSDate.date;
    [self handleAppConfirmationChange:self.triggerDate];
}
- (void)handleAppConfirmationChange:(id)payload {
    if (![payload isEqual:self.triggerDate]) return;
    [self stopInput]; self.observing = YES;
    for (NSString *key in @[HarnessBackgroundResult, HarnessAudioLevel])
        [VoicePreferences() addObserver:self forKeyPath:key options:NSKeyValueObservingOptionNew context:NULL];
    __weak typeof(self) weakSelf = self;
    self.startupTimer = [NSTimer scheduledTimerWithTimeInterval:3 repeats:NO block:^(NSTimer *timer) {
        GKBKeyboardVoiceInput *keyboard = weakSelf;
        if (!keyboard) return;
        keyboard.timeouts++;
        [keyboard.monitor handleKeyboardCancelChange:nil];
        [keyboard stopInput];
    }];
}
- (void)stopInput {
    [self.startupTimer invalidate]; self.startupTimer = nil;
    if (!self.observing) return;
    self.observing = NO;
    for (NSString *key in @[HarnessBackgroundResult, HarnessAudioLevel])
        [VoicePreferences() removeObserver:self forKeyPath:key];
}
- (void)didReceiveValidResponse { [self.startupTimer invalidate]; self.startupTimer = nil; }
- (void)handleAudioLevelChange:(NSDictionary *)change {
    self.lastAudioLevel = [change[NSKeyValueChangeNewKey] doubleValue];
    // The inspected device binary uses this threshold, including its watchdog.
    if (self.lastAudioLevel > 0.4 && self.startupTimer) [self didReceiveValidResponse];
}
- (void)observeValueForKeyPath:(NSString *)key ofObject:(id)object change:(NSDictionary *)change context:(void *)context {
    if ([key isEqual:HarnessBackgroundResult]) [self handleTextResultChange:change];
    else if ([key isEqual:HarnessAudioLevel]) [self handleAudioLevelChange:change];
}
- (void)voiceInputOverlayDidTapCloseButton:(id)overlay {
    [self stopInput]; [self finishWithResult:nil error:nil];
}
- (void)voiceInputOverlayDidTapStopButton:(id)overlay {
    [self didReceiveValidResponse];
    [self.monitor handleKeyboardCancelChange:nil];
}
- (void)voiceInputOverlayDidTapMicButton:(id)overlay { self.originalMicStarts++; }
- (void)handleTextResultChange:(NSDictionary *)change {
    NSAssert([change isKindOfClass:NSDictionary.class], @"Result callbacks require a KVO change dictionary");
    self.resultChanges++;
    [self didReceiveValidResponse];
    NSString *text = change[NSKeyValueChangeNewKey];
    if ([text isKindOfClass:NSString.class] && text.length && ![text isEqual:kGKBVoiceInputErrorString])
        [self updateDocumentProxyWithText:text];
    [self stopInput];
}
- (void)updateDocumentProxyWithText:(NSString *)text { [self.document insertText:text]; }
- (void)dealloc { [self stopInput]; }
@end

@interface GKBSuiteName : NSObject
+ (NSString *)suiteName;
+ (NSString *)googleCommonSuiteName;
@end
@implementation GKBSuiteName
+ (NSString *)suiteName { return @"group.com.google.keyboard"; }
+ (NSString *)googleCommonSuiteName { return @"group.com.google.common"; }
@end

@interface GKBAppURLSchemeUtilities : NSObject
+ (NSURL *)GboardURLWithHost:(NSString *)host path:(NSString *)path query:(NSString *)query;
+ (BOOL)GboardCanOpenURL:(NSURL *)url;
+ (NSURL *)voiceInputURL;
@end
@implementation GKBAppURLSchemeUtilities
+ (NSURL *)GboardURLWithHost:(NSString *)host path:(NSString *)path query:(NSString *)query {
    NSURLComponents *parts = [NSURLComponents new];
    parts.scheme = @"gboard"; parts.host = host; parts.path = path; parts.query = query;
    return parts.URL;
}
+ (BOOL)GboardCanOpenURL:(NSURL *)url { return [url.scheme isEqual:@"gboard"]; }
+ (NSURL *)voiceInputURL { return [self GboardURLWithHost:@"voice" path:@"/dictate" query:nil]; }
@end

// Model the inspected foreground lifecycle and keyboard acceptance rule. These
// fixture keys are private to this harness; no Google implementation is included.
static NSString *const HarnessResult = @"HarnessDictationResult";
static NSString *const HarnessEndsTime = @"HarnessDictationEndsTime";
static NSUserDefaults *VoicePreferences(void) {
    static NSUserDefaults *preferences;
    if (!preferences) preferences = [[NSUserDefaults alloc] initWithSuiteName:[GKBSuiteName suiteName]];
    return preferences;
}
@interface GKBVoiceRecognitionViewController : NSObject
@property(nonatomic, copy) NSString *currentResult;
@property(nonatomic) NSUInteger cleanups, endTimeWrites;
- (void)recordVoiceInputEndsTime;
@end
@implementation GKBVoiceRecognitionViewController
- (void)glifVoiceSearchViewController:(id)controller didReceiveResult:(NSString *)text {
    self.currentResult = text;
    [VoicePreferences() setObject:text forKey:HarnessResult];
    [self performSelector:@selector(dismissAndNavigateBackToPreviousApp) withObject:nil afterDelay:0.5];
}
- (void)recordVoiceInputEndsTime {
    self.endTimeWrites++;
    [VoicePreferences() setDouble:CACurrentMediaTime() forKey:HarnessEndsTime];
}
- (void)dismissAndNavigateBackToPreviousApp {
    __weak typeof(self) owner = self;
    [[GKBAppNavigation new] navigateBackToPreviousAppWithStartBlock:^{
        [owner recordVoiceInputEndsTime];
    } cleanupBlock:^{
        owner.cleanups++;
        owner.currentResult = nil;
    }];
}
@end

@interface HarnessKeyboard : NSObject
@property(nonatomic, strong) UITextView *document;
@property(nonatomic) BOOL voiceInputEnabled;
- (BOOL)handleVoiceInput;
@end
@implementation HarnessKeyboard
- (instancetype)init {
    if ((self = [super init])) { self.document = [UITextView new]; self.voiceInputEnabled = YES; }
    return self;
}
- (BOOL)handleVoiceInput {
    NSUserDefaults *preferences = VoicePreferences();
    NSString *text = [preferences stringForKey:HarnessResult];
    BOOL accepted = self.voiceInputEnabled &&
        CACurrentMediaTime() - [preferences doubleForKey:HarnessEndsTime] < 10 && text.length > 0;
    if (accepted) [self.document insertText:text];
    [preferences removeObjectForKey:HarnessResult];
    return accepted;
}
@end

@interface AppDelegate : UIResponder <UIApplicationDelegate>
@property(nonatomic, strong) UIWindow *window;
@property(nonatomic, strong) UITextView *output;
@property(nonatomic, strong) GSKSpeechToTextStreamingController *voice, *cancelled;
@property(nonatomic, strong) GKBVoiceRecognitionViewController *recognition;
@property(nonatomic, strong) HarnessKeyboard *keyboard;
@property(nonatomic, strong) GKBVoiceInputMonitor *backgroundMonitor;
@property(nonatomic, strong) GKBKeyboardVoiceInput *backgroundKeyboard;
@property(nonatomic, strong) NSDate *deadline;
@property(nonatomic, strong) NSURL *receivedURL;
@property(nonatomic, strong) NSMutableArray<NSString *> *checks;
- (void)beginWithWindow:(UIWindow *)window;
@end
@implementation AppDelegate
- (BOOL)application:(UIApplication *)app openURL:(NSURL *)url options:(NSDictionary *)options {
    self.receivedURL = url; return YES;
}
- (void)check:(BOOL)passed message:(NSString *)message {
    if (!passed) [NSException raise:@"HarnessFailure" format:@"%@", message];
    [self.checks addObject:message];
}
- (GSKSpeechToTextStreamingController *)newVoice {
    GSKSpeechToTextStreamingController *value = [GSKSpeechToTextStreamingController new];
    value.speechSessionConfig = [GKBVoiceInputSpeechSessionConfig new]; return value;
}
- (UIView *)viewWithIdentifier:(NSString *)identifier inView:(UIView *)view {
    if ([view.accessibilityIdentifier isEqual:identifier]) return view;
    for (UIView *child in view.subviews) {
        UIView *match = [self viewWithIdentifier:identifier inView:child];
        if (match) return match;
    }
    return nil;
}
- (UIView *)returnGuide {
    return [self viewWithIdentifier:@"kiwi.rambler.return-guide" inView:self.window.rootViewController.view];
}
- (NSString *)returnGuideStatus {
    UILabel *label = (UILabel *)[self viewWithIdentifier:@"kiwi.rambler.return-guide.status" inView:[self returnGuide]];
    return label.text;
}
- (void)runNavigationChecks {
    NSUserDefaults *preferences = [[NSUserDefaults alloc] initWithSuiteName:[GKBSuiteName suiteName]];
    [preferences setBool:NO forKey:@"KiwiRamblerInKeyboard"];
    [preferences setBool:NO forKey:@"KiwiRamblerStandard"];
    GKBFeatureFlagManager *flags = [GKBFeatureFlagManager new];
    [self check:[flags enableInKeyboardVoiceInput] == flags.voiceFlag message:@"Preserve the original feature flag by default"];
    [preferences setBool:YES forKey:@"KiwiRamblerInKeyboard"];
    GKBBoolFeatureFlag *enabled = [flags enableInKeyboardVoiceInput];
    [self check:[enabled value] && [[enabled marker] isEqual:@"original flag"] && !flags.unrelatedFlag.value message:@"Enable only in-keyboard voice input and forward other flag selectors"];
    [preferences setBool:NO forKey:@"KiwiRamblerInKeyboard"];
    [self check:![enabled value] && [flags enableInKeyboardVoiceInput] == flags.voiceFlag message:@"Restore the original feature value when the setting is disabled"];
    HarnessApplication *app = (HarnessApplication *)UIApplication.sharedApplication;
    __block NSUInteger missingStarts = 0, missingCleanups = 0, emptyCleanups = 0, pendingCleanups = 0, rejectedCleanups = 0;
    __block NSUInteger successStarts = 0, successCleanups = 0, droppedCleanups = 0, lateStarts = 0, lateCleanups = 0;
    app.returnAction = nil;
    GKBAppNavigation *missing = [GKBAppNavigation new];
    NSMutableArray *order = [NSMutableArray array];
    [self check:![missing navigateBackToPreviousAppWithStartBlock:^{ missingStarts++; [order addObject:@"start"]; } cleanupBlock:^{ missingCleanups++; [order addObject:@"cleanup"]; }] message:@"Report a missing iOS return route"];
    HarnessNavigationAction *action = [HarnessNavigationAction new];
    action.destinations = @[]; app.returnAction = action;
    GKBAppNavigation *empty = [GKBAppNavigation new];
    [empty navigateBackToPreviousAppWithStartBlock:nil cleanupBlock:^{ emptyCleanups++; }];
    action.destinations = @[@1];
    GKBAppNavigation *pending = [GKBAppNavigation new]; pending.hasPendingNavigation = YES;
    [pending navigateBackToPreviousAppWithStartBlock:nil cleanupBlock:^{ pendingCleanups++; }];
    GKBAppNavigation *rejected = [GKBAppNavigation new]; rejected.accepts = NO;
    [rejected navigateBackToPreviousAppWithStartBlock:nil cleanupBlock:^{ rejectedCleanups++; }];
    GKBAppNavigation *success = [GKBAppNavigation new]; success.repeatsCallbacks = YES;
    [self check:[success navigateBackToPreviousAppWithStartBlock:^{ successStarts++; } cleanupBlock:^{ successCleanups++; }] message:@"Preserve an available iOS return route"];
    GKBAppNavigation *dropped = [GKBAppNavigation new]; dropped.dropsCallbacks = YES;
    [dropped navigateBackToPreviousAppWithStartBlock:nil cleanupBlock:^{ droppedCleanups++; }];
    GKBAppNavigation *late = [GKBAppNavigation new]; late.callbackDelay = 1.2;
    [late navigateBackToPreviousAppWithStartBlock:^{ lateStarts++; } cleanupBlock:^{ lateCleanups++; }];
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 1500 * NSEC_PER_MSEC), dispatch_get_main_queue(), ^{
        @try {
            [self check:missing.calls == 0 && missingStarts == 1 && missingCleanups == 1 && [order isEqual:@[@"start", @"cleanup"]] message:@"Publish lifecycle completion before cleanup without an OS return route"];
            [self check:empty.calls == 0 && emptyCleanups == 1 message:@"Handle an empty destination list"];
            [self check:pending.calls == 0 && pendingCleanups == 1 message:@"Clean up an already pending navigation"];
            [self check:rejected.calls == 1 && rejectedCleanups == 1 message:@"Clean up a rejected navigation"];
            [self check:success.calls == 1 && successStarts == 1 && successCleanups == 1 message:@"Run navigation callbacks exactly once"];
            [self check:dropped.calls == 1 && droppedCleanups == 1 message:@"Recover when accepted navigation drops its callback"];
            [self check:late.calls == 1 && lateStarts == 1 && lateCleanups == 1 message:@"Complete the lifecycle once and ignore late navigation callbacks"];
            [self runKeyboardUIChecks];
        } @catch (NSException *error) { self.output.text = [@"FAIL: " stringByAppendingString:error.reason]; }
    });
}
- (GKBKeyboardVoiceInput *)newKeyboardFixture {
    GKBKeyboardVoiceInput *keyboard = [GKBKeyboardVoiceInput new];
    keyboard.document = [UITextView new];
    GKBVoiceInputLauncher *launcher = [GKBVoiceInputLauncher new];
    keyboard.fixtureLauncher = launcher;
    launcher.delegate = [GKBUIInputViewController new];
    launcher.placeholderOverlay = [UIView new];
    [launcher.delegate voiceInputLauncher:launcher showVoiceInputOverlay:launcher.placeholderOverlay];
    keyboard.overlay = [UIView new];
    [launcher.delegate voiceInputLauncher:launcher showVoiceInputOverlay:keyboard->_overlay];
    __weak GKBVoiceInputLauncher *weakLauncher = launcher;
    keyboard.completion = ^(id result, NSError *error) { [weakLauncher completionHandler:result error:error]; };
    return keyboard;
}
- (void)runKeyboardUIChecks {
    [VoicePreferences() setBool:YES forKey:@"KiwiRamblerInKeyboard"];
    if (PublishCaptureState) PublishCaptureState(YES);
    NSDictionary *success = @{NSKeyValueChangeNewKey:@"Keep the complete sentence."};
    GKBKeyboardVoiceInput *keyboard = [self newKeyboardFixture];
    UIView *voiceOverlay = keyboard->_overlay, *placeholder = keyboard.fixtureLauncher.placeholderOverlay;
    [keyboard start];
    [self check:keyboard->_overlay.superview != nil && keyboard.finishCalls == 0 && keyboard.observing message:@"Confirmation stopInput keeps the voice UI open while listening"];
    [keyboard handleTextResultChange:success];
    [self check:[keyboard.document.text isEqual:success[NSKeyValueChangeNewKey]] && !keyboard.observing && keyboard.finishCalls == 0 message:@"Insert and unsubscribe before asynchronously completing the voice UI"];

    // On foreground return the launcher may transfer its attached placeholder
    // to the keyboard session rather than create a second overlay.
    GKBKeyboardVoiceInput *transferred = [self newKeyboardFixture];
    [transferred->_overlay removeFromSuperview];
    transferred.overlay = transferred.fixtureLauncher.placeholderOverlay;
    [transferred start]; [transferred handleTextResultChange:success];

    NSMutableArray<GKBKeyboardVoiceInput *> *retryable = [NSMutableArray array];
    for (id value in @[kGKBVoiceInputErrorString, @"", NSNull.null]) {
        GKBKeyboardVoiceInput *retry = [self newKeyboardFixture]; [retry start];
        [retry handleTextResultChange:@{NSKeyValueChangeNewKey:value}]; [retryable addObject:retry];
    }
    GKBKeyboardVoiceInput *disabled = [self newKeyboardFixture]; [disabled start];
    [VoicePreferences() setBool:NO forKey:@"KiwiRamblerInKeyboard"];
    [disabled handleTextResultChange:success];
    [VoicePreferences() setBool:YES forKey:@"KiwiRamblerInKeyboard"];

    GKBKeyboardVoiceInput *closed = [self newKeyboardFixture]; [closed start];
    [closed handleTextResultChange:success]; [closed voiceInputOverlayDidTapCloseButton:nil];
    GKBKeyboardVoiceInput *restarted = [self newKeyboardFixture]; [restarted start];
    [restarted handleTextResultChange:success]; [restarted start];
    GKBKeyboardVoiceInput *mic = [self newKeyboardFixture]; [mic start];
    [mic handleTextResultChange:success]; [mic voiceInputOverlayDidTapMicButton:nil];
    GKBKeyboardVoiceInput *completed = [self newKeyboardFixture]; [completed start];
    [completed handleTextResultChange:success]; [completed finishWithResult:nil error:nil];

    GKBKeyboardVoiceInput *missingOverlay = [self newKeyboardFixture]; [missingOverlay start];
    [missingOverlay->_overlay removeFromSuperview]; missingOverlay->_overlay = nil;
    [missingOverlay handleTextResultChange:success];
    GKBKeyboardVoiceInput *missingCompletion = [self newKeyboardFixture]; [missingCompletion start];
    missingCompletion.completion = nil; [missingCompletion handleTextResultChange:success];
    GKBKeyboardVoiceInput *replaced = [self newKeyboardFixture]; [replaced start];
    [replaced handleTextResultChange:success];
    [replaced->_overlay removeFromSuperview]; replaced.overlay = [UIView new];
    [replaced.fixtureLauncher.delegate voiceInputLauncher:replaced.fixtureLauncher showVoiceInputOverlay:replaced->_overlay];

    dispatch_async(dispatch_get_main_queue(), ^{
        @try {
            UIView *root = keyboard.fixtureLauncher.delegate.view;
            [self check:keyboard.finishCalls == 1 && keyboard.cleanups == 1 && keyboard->_overlay == nil && keyboard->_completion == nil && root.subviews.count == 0 message:@"Successful insertion removes the voice overlay and launcher placeholder"];
            [self check:keyboard.fixtureLauncher.completions == 1 && keyboard.fixtureLauncher.delegate.cursorNotifications == 1 && [keyboard.document.text isEqual:success[NSKeyValueChangeNewKey]] message:@"Complete the launcher and notify cursor movement once without reinserting text"];
            BOOL removedConstraints = YES;
            for (NSLayoutConstraint *constraint in root.constraints)
                removedConstraints &= constraint.firstItem != voiceOverlay && constraint.secondItem != voiceOverlay &&
                    constraint.firstItem != placeholder && constraint.secondItem != placeholder;
            [self check:removedConstraints && !keyboard.overlayGetterCalls message:@"Release overlay constraints without lazy overlay creation or fixed keyboard sizing"];
            [self check:transferred.finishCalls == 1 && transferred.fixtureLauncher.completions == 1 && transferred.fixtureLauncher.delegate.view.subviews.count == 0 message:@"Remove a launcher placeholder transferred to the keyboard as the same overlay"];
            BOOL keptRetryUI = YES;
            for (GKBKeyboardVoiceInput *retry in retryable)
                keptRetryUI &= retry.finishCalls == 0 && retry->_overlay.superview != nil && retry.document.text.length == 0;
            [self check:keptRetryUI message:@"Errors and empty results retain retry controls without inserting text"];
            [self check:disabled.finishCalls == 0 && disabled->_overlay.superview != nil message:@"Leave the original UI lifecycle unchanged when experimental mode is disabled"];
            [self check:closed.finishCalls == 1 && completed.finishCalls == 1 message:@"Do not complete twice after Close or native completion beats the queued callback"];
            [self check:restarted.finishCalls == 0 && restarted.observing && restarted->_overlay.superview != nil message:@"A queued result cannot close a new recording on the same voice-input object"];
            [self check:mic.finishCalls == 0 && mic.originalMicStarts == 1 && mic->_overlay.superview != nil message:@"A microphone retry invalidates the previous queued UI completion"];
            [self check:missingOverlay.finishCalls == 0 && !missingOverlay.overlayGetterCalls && missingCompletion.finishCalls == 0 message:@"Require an existing overlay and completion without invoking lazy getters"];
            [self check:replaced.finishCalls == 0 && replaced->_overlay.superview != nil message:@"Do not close an overlay replaced before the result callback runs"];
            for (GKBKeyboardVoiceInput *fixture in @[disabled, restarted, mic, missingOverlay, missingCompletion, replaced]) {
                [fixture stopInput]; [fixture finishWithResult:nil error:nil];
            }
            for (GKBKeyboardVoiceInput *retry in retryable) [retry finishWithResult:nil error:nil];
            if (PublishCaptureState) PublishCaptureState(NO);
            [self runCaptureReadinessCheck];
        } @catch (NSException *error) { self.output.text = [@"FAIL: " stringByAppendingString:error.reason]; NSLog(@"%@", self.output.text); }
    });
}
- (void)runCaptureReadinessCheck {
    NSUserDefaults *preferences = [[NSUserDefaults alloc] initWithSuiteName:[GKBSuiteName suiteName]];
    [preferences setBool:YES forKey:@"KiwiRamblerInKeyboard"];
    setenv("KIWI_RAMBLER_TEST_CAPTURE_DELAY", "0.2", 1);
    GSKSpeechToTextStreamingController *voice = [self newVoice];
    [voice startVoiceSearch];
    GKBAppNavigation *navigation = [GKBAppNavigation new];
    __block NSUInteger cleanups = 0;
    [navigation navigateBackToPreviousAppWithStartBlock:nil cleanupBlock:^{ cleanups++; }];
    [self check:navigation.calls == 0 message:@"Wait for microphone readiness before navigating"];
    [self check:[self returnGuide] == nil message:@"Do not tell the user to switch apps before the microphone is ready"];
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 350 * NSEC_PER_MSEC), dispatch_get_main_queue(), ^{
        @try {
            [self check:navigation.calls == 1 && cleanups == 1 message:@"Navigate after capture has started"];
            ((HarnessApplication *)UIApplication.sharedApplication).returnAction = nil;
            GKBAppNavigation *manual = [GKBAppNavigation new];
            __block NSUInteger activations = 0, manualCleanups = 0;
            [manual navigateBackToPreviousAppWithStartBlock:^{ activations++; } cleanupBlock:^{ manualCleanups++; }];
            dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 300 * NSEC_PER_MSEC), dispatch_get_main_queue(), ^{
                @try {
                    [self check:manual.calls == 0 && activations == 1 && manualCleanups == 1 message:@"Announce ready background capture even when return must be manual"];
                    UIView *guide = [self returnGuide];
                    [self check:guide && [[self returnGuideStatus] isEqual:@"Microphone ready"] && voice.isPerformingVoiceSearch message:@"Show swipe-back guidance while the ready microphone keeps recording"];
                    [self.window.rootViewController.view layoutIfNeeded];
                    UILabel *heading = (UILabel *)[self viewWithIdentifier:@"kiwi.rambler.return-guide.title" inView:guide];
                    UILabel *instructions = (UILabel *)[self viewWithIdentifier:@"kiwi.rambler.return-guide.instructions" inView:guide];
                    CGRect headingRect = [heading convertRect:heading.bounds toView:guide];
                    CGRect instructionsRect = [instructions convertRect:instructions.bounds toView:guide];
                    [self check:CGRectEqualToRect(guide.frame, self.window.rootViewController.view.bounds) && CGRectContainsRect(guide.bounds, headingRect) && CGRectContainsRect(guide.bounds, instructionsRect) && heading.bounds.size.width <= guide.bounds.size.width - 47 message:@"Keep the complete return screen and wrapped instructions inside the phone width"];
                    [self check:self.window.rootViewController.presentedViewController == nil && self.window.rootViewController.childViewControllers.count == 1 message:@"Use one contained return screen without an alert or dismissal sheet"];
                    UILabel *gesture = (UILabel *)[self viewWithIdentifier:@"kiwi.rambler.return-guide.gesture" inView:guide];
                    [self check:[gesture.text containsString:@"bottom edge"] && [gesture.text containsString:@"app switcher"] message:@"Describe the system swipe and app-switcher alternative"];
                    [NSNotificationCenter.defaultCenter postNotificationName:UIApplicationDidEnterBackgroundNotification object:UIApplication.sharedApplication];
                    [self check:[self returnGuide] == nil && voice.isPerformingVoiceSearch message:@"Leaving the app removes guidance without stopping background recording"];
                    [NSNotificationCenter.defaultCenter postNotificationName:UIApplicationDidBecomeActiveNotification object:UIApplication.sharedApplication];
                    [self check:[self returnGuide] == nil message:@"Do not show stale return guidance on an unrelated foreground activation"];
                    GKBVoiceInputMonitor *monitor = [GKBVoiceInputMonitor new]; monitor->_streamingController = voice; [monitor stop];
                    GSKSpeechToTextStreamingController *cancelled = [self newVoice]; [cancelled startVoiceSearch];
                    GKBAppNavigation *abandoned = [GKBAppNavigation new];
                    __block NSUInteger cancelledStarts = 0, cancelledCleanups = 0;
                    [abandoned navigateBackToPreviousAppWithStartBlock:^{ cancelledStarts++; } cleanupBlock:^{ cancelledCleanups++; }];
                    monitor->_streamingController = cancelled; [monitor stop];
                    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 350 * NSEC_PER_MSEC), dispatch_get_main_queue(), ^{
                        @try {
                            [self check:abandoned.calls == 0 && cancelledStarts == 0 && cancelledCleanups == 1 && !cancelled.isPerformingVoiceSearch message:@"Cancel a handoff without late capture, activation or navigation"];
                            [self check:[self returnGuide] == nil message:@"A cancelled handoff cannot display delayed swipe-back guidance"];
                            unsetenv("KIWI_RAMBLER_TEST_CAPTURE_DELAY");
                            [preferences setBool:NO forKey:@"KiwiRamblerInKeyboard"];
                            [self runSpeechChecks];
                        } @catch (NSException *error) { self.output.text = [@"FAIL: " stringByAppendingString:error.reason]; }
                    });
                } @catch (NSException *error) { self.output.text = [@"FAIL: " stringByAppendingString:error.reason]; }
            });
        } @catch (NSException *error) { self.output.text = [@"FAIL: " stringByAppendingString:error.reason]; }
    });
}
- (void)run {
    @try {
        NSString *library = [NSBundle.mainBundle.bundlePath stringByAppendingPathComponent:@"Frameworks/KiwiGboardRambler.dylib"];
        void *handle = dlopen(library.UTF8String, RTLD_NOW);
        [self check:handle != NULL message:@"Load the real native client"];
        LastDictation = (NSString *(*)(void))dlsym(handle, "KGRLastDictation");
        DiagnosticReport = (NSString *(*)(void))dlsym(handle, "KGRDiagnosticsReport");
        RecordDiagnostic = (void (*)(NSString *, NSDictionary *))dlsym(handle, "KGRRecordEvent");
        PublishCaptureState = (void (*)(BOOL))dlsym(handle, "KGRPublishCaptureState");
        void (*clear)(void) = (void (*)(void))dlsym(handle, "KGRClearDiagnostics");
        if (clear) clear();
        typedef NSString *(*GroupFunction)(NSString *);
        GroupFunction group = (GroupFunction)dlsym(handle, "KGRGroup");
        [self check:group && group(nil) == nil && [group(@"unrelated") isEqual:@"unrelated"] message:@"Preserve nil and unrelated suites"];
        NSString *mapped = @"group.com.example.kiwi.keyboard";
        [self check:[[GKBSuiteName suiteName] isEqual:mapped] message:@"Resolve Gboard's shared suite alias"];
        NSUserDefaults *original = [[NSUserDefaults alloc] initWithSuiteName:@"group.com.google.keyboard"];
        [original setObject:@"shared" forKey:@"HarnessSharedValue"];
        NSUserDefaults *owned = [[NSUserDefaults alloc] initWithSuiteName:mapped];
        [self check:[[owned stringForKey:@"HarnessSharedValue"] isEqual:@"shared"] message:@"Remap Foundation suite access"];
        NSURL *url = [GKBAppURLSchemeUtilities GboardURLWithHost:@"voice" path:@"/test" query:@"a=b"];
        [self check:[url.absoluteString isEqual:@"kiwi-harness://voice/test?a=b"] && [GKBAppURLSchemeUtilities GboardCanOpenURL:url] message:@"Use the containing app URL without losing its route"];
        [self application:UIApplication.sharedApplication openURL:url options:@{}];
        [self check:[self.receivedURL.absoluteString isEqual:@"gboard://voice/test?a=b"] message:@"Translate incoming URLs for Gboard's router"];
        [self runNavigationChecks];
    } @catch (NSException *error) { self.output.text = [@"FAIL: " stringByAppendingString:error.reason]; }
}
- (void)runSpeechChecks {
    @try {
        GSKSpeechToTextStreamingController *other = [GSKSpeechToTextStreamingController new];
        [other startVoiceSearch];
        [self check:other.originalStarts == 1 && !other.canPerformVoiceSearch && [other.recognitionResult isEqual:@"original"] message:@"Preserve unrelated recognizers"];
        self.cancelled = [self newVoice];
        [self.cancelled startVoiceSearch];
        [[GKBKeyboardVoiceInput new] voiceInputOverlayDidTapCloseButton:nil];
        [self.cancelled stopSpeechRecognition];
        [self check:!self.cancelled.isPerformingVoiceSearch message:@"Close cancels capture"];
        [self.cancelled startVoiceSearch];
        [self.cancelled stopSpeechRecognition];
        GKBVoiceInputMonitor *monitor = [GKBVoiceInputMonitor new];
        monitor->_streamingController = self.cancelled;
        [monitor stop];
        [self check:!self.cancelled.isPerformingVoiceSearch && monitor.getterCalls == 0 message:@"Monitor shutdown cancels pending work without invoking its getter"];
        self.voice = [self newVoice];
        self.recognition = [GKBVoiceRecognitionViewController new];
        self.voice.delegate = self.recognition;
        self.keyboard = [HarnessKeyboard new];
        [VoicePreferences() removeObjectForKey:HarnessResult];
        [VoicePreferences() setDouble:0 forKey:HarnessEndsTime];
        [self.voice startVoiceSearch];
        [self check:self.voice.isPerformingVoiceSearch && self.voice.originalStarts == 0 message:@"Start Rambler for the keyboard voice configuration"];
        GKBVoiceInputMonitor *meter = [GKBVoiceInputMonitor new]; meter->_streamingController = self.voice;
        HarnessAudioSource *legacy = [HarnessAudioSource new];
        [meter startMonitoringAudioLevelWithSource:legacy]; [meter updateAudioLevel];
        [self check:meter.lastAudioLevel == self.voice.currentAudioLevel && meter.lastAudioLevel > 0 && meter.lastAudioLevel != legacy.currentAudioLevel && meter.getterCalls == 0 message:@"Drive the background keyboard meter from Rambler's recorder"];
        GKBVoiceInputMonitor *unrelated = [GKBVoiceInputMonitor new];
        [unrelated startMonitoringAudioLevelWithSource:legacy]; [unrelated updateAudioLevel];
        [self check:unrelated.lastAudioLevel == legacy.currentAudioLevel message:@"Preserve the audio meter for unrelated capture sources"];
        [unrelated startMonitoringAudioLevelWithSource:nil];
        [self check:unrelated.audioSource == nil message:@"Preserve a nil audio source during monitor teardown"];
        [[GKBKeyboardVoiceInput new] voiceInputOverlayDidTapStopButton:nil];
        [self.voice stopSpeechRecognition];
        self.deadline = [NSDate dateWithTimeIntervalSinceNow:60];
        [NSTimer scheduledTimerWithTimeInterval:0.1 target:self selector:@selector(poll:) userInfo:nil repeats:YES];
    } @catch (NSException *error) { self.output.text = [@"FAIL: " stringByAppendingString:error.reason]; }
}
- (void)poll:(NSTimer *)timer {
    if (!self.voice.deliveries && !self.voice.error && self.deadline.timeIntervalSinceNow > 0) return;
    [timer invalidate];
    @try {
        [self check:self.voice.error == nil && self.voice.deliveries == 1 message:@"Deliver one completed speech result through the Gboard callback"];
        NSString *text = self.voice.deliveredText.lowercaseString;
        [self check:[text containsString:@"wednesday"] && ![text containsString:@"tuesday"] message:@"Apply the spoken correction using the live service"];
        [self check:!self.voice.isPerformingVoiceSearch && [self.voice.entireCurrentResult isEqual:self.voice.deliveredText] message:@"Expose the final transcript to Gboard"];
        [self check:self.cancelled.deliveries == 0 && self.cancelled.error == nil message:@"Suppress delivery after cancellation"];
        ((HarnessApplication *)UIApplication.sharedApplication).returnAction = nil;
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 1900 * NSEC_PER_MSEC), dispatch_get_main_queue(), ^{
            @try {
                [self check:self.recognition.cleanups == 1 && self.recognition.currentResult == nil && self.window.rootViewController.presentedViewController == nil message:@"Clean up the foreground controller without a return-to-app popup"];
                [self check:[[self returnGuideStatus] isEqual:@"Your text is ready"] message:@"Show completed-dictation guidance after foreground cleanup without a popup"];
                [self check:[self.voice.entireCurrentResult isEqual:self.voice.deliveredText] && self.voice.deliveries == 1 message:@"Preserve the transcript without duplicate delivery during fallback"];
                [self check:LastDictation && [LastDictation() isEqual:self.voice.deliveredText] message:@"Retain the latest result for explicit copying in Rambler settings"];
                [self check:self.recognition.endTimeWrites == 1 message:@"Publish the foreground completion timestamp exactly once before cleanup"];
                [self check:[self.keyboard handleVoiceInput] && [self.keyboard.document.text isEqual:self.voice.deliveredText] message:@"Insert corrected text on keyboard return after the recognition UI was dismissed"];
                [self check:![self.keyboard handleVoiceInput] && [self.keyboard.document.text isEqual:self.voice.deliveredText] message:@"Consume a result once without inserting it twice"];
                [VoicePreferences() setObject:self.voice.deliveredText forKey:HarnessResult];
                [VoicePreferences() setDouble:CACurrentMediaTime() - 60 forKey:HarnessEndsTime];
                [self check:![[HarnessKeyboard new] handleVoiceInput] message:@"Keep Gboard's stale-result rejection intact"];
                [VoicePreferences() setObject:self.voice.deliveredText forKey:HarnessResult];
                [NSNotificationCenter.defaultCenter postNotificationName:UIApplicationWillResignActiveNotification object:UIApplication.sharedApplication];
                HarnessKeyboard *returned = [HarnessKeyboard new];
                [self check:self.recognition.endTimeWrites == 2 && [returned handleVoiceInput] && [returned.document.text isEqual:self.voice.deliveredText] message:@"Refresh the validity window when manually leaving the completed dictation"];
                [NSNotificationCenter.defaultCenter postNotificationName:UIApplicationDidEnterBackgroundNotification object:UIApplication.sharedApplication];
                [self check:[self returnGuide] == nil message:@"Remove completed guidance as the user returns to the original app"];
                [VoicePreferences() setDouble:42 forKey:HarnessEndsTime];
                [NSNotificationCenter.defaultCenter postNotificationName:UIApplicationWillResignActiveNotification object:UIApplication.sharedApplication];
                [self check:self.recognition.endTimeWrites == 2 && [VoicePreferences() doubleForKey:HarnessEndsTime] == 42 message:@"Release the pending handoff after returning so it cannot refresh another result"];
                self.recognition.currentResult = self.voice.deliveredText;
                [self.recognition recordVoiceInputEndsTime];
                self.recognition.currentResult = nil;
                GSKSpeechToTextStreamingController *next = [self newVoice]; [next startVoiceSearch];
                GKBVoiceInputMonitor *monitor = [GKBVoiceInputMonitor new]; monitor->_streamingController = next; [monitor stop];
                [VoicePreferences() setDouble:42 forKey:HarnessEndsTime];
                [NSNotificationCenter.defaultCenter postNotificationName:UIApplicationWillResignActiveNotification object:UIApplication.sharedApplication];
                [self check:self.recognition.endTimeWrites == 3 && [VoicePreferences() doubleForKey:HarnessEndsTime] == 42 message:@"A new dictation invalidates the previous pending return"];
                [VoicePreferences() setObject:self.voice.deliveredText forKey:HarnessResult];
                [VoicePreferences() setDouble:CACurrentMediaTime() forKey:HarnessEndsTime];
                HarnessKeyboard *disabled = [HarnessKeyboard new]; disabled.voiceInputEnabled = NO;
                [self check:![disabled handleVoiceInput] && disabled.document.text.length == 0 message:@"Respect disabled keyboard voice input"];
                [self runBackgroundStopCheck];
            } @catch (NSException *error) { self.output.text = [@"FAIL: " stringByAppendingString:error.reason]; }
            NSLog(@"%@", self.output.text);
        });
    } @catch (NSException *error) { self.output.text = [@"FAIL: " stringByAppendingString:error.reason]; }
    NSLog(@"%@", self.output.text);
}
- (void)runBackgroundStopCheck {
    self.output.text = @"Checking the quiet-input watchdog and background result observation…";
    [VoicePreferences() setBool:YES forKey:@"KiwiRamblerInKeyboard"];
    self.backgroundMonitor = [GKBVoiceInputMonitor new];
    self.backgroundKeyboard = [self newKeyboardFixture];
    self.backgroundKeyboard.monitor = self.backgroundMonitor;
    __weak GSKSpeechToTextStreamingController *weakController;
    @autoreleasepool {
        GSKSpeechToTextStreamingController *voice = [self newVoice];
        voice.delegate = self.backgroundMonitor;
        // The inspected controller retains its volume factory, while its delegate
        // and Rambler's controller reference are weak. Stop tears down the owner.
        voice.speechVolumeStreamFactory = self.backgroundMonitor;
        self.backgroundMonitor->_streamingController = voice;
        weakController = voice;
        [voice startVoiceSearch];
        [self.backgroundKeyboard start];
        [[GKBAppNavigation new] navigateBackToPreviousAppWithStartBlock:nil cleanupBlock:nil];
        [self check:self.backgroundKeyboard.startupTimer == nil message:@"Acknowledge a ready host handshake without waiting for loud audio"];
        // Real phone speech was -36 dB (raw amplitude ~0.016). A ready but
        // silent recorder must survive the same three-second startup window.
        [self.backgroundKeyboard handleAudioLevelChange:@{NSKeyValueChangeNewKey:@0.016}];
    }
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 3200 * NSEC_PER_MSEC), dispatch_get_main_queue(), ^{
        @try {
            [self check:self.backgroundKeyboard.timeouts == 0 && self.backgroundKeyboard.observing && self.backgroundKeyboard.lastAudioLevel == 0.016 message:@"Keep quiet background capture subscribed beyond Gboard's three-second watchdog"];
            if (PublishCaptureState) PublishCaptureState(YES);
            [self.backgroundKeyboard voiceInputOverlayDidTapStopButton:nil];
            [self check:[[self returnGuideStatus] isEqual:@"Finishing your dictation…"] message:@"Update visible guidance when recording stops and transcription begins"];
            [self check:self.backgroundMonitor->_streamingController == nil && weakController != nil message:@"Retain the asynchronous delivery target after background Stop releases its controller"];
            self.deadline = [NSDate dateWithTimeIntervalSinceNow:60];
            [NSTimer scheduledTimerWithTimeInterval:0.1 target:self selector:@selector(pollBackground:) userInfo:nil repeats:YES];
        } @catch (NSException *error) { self.output.text = [@"FAIL: " stringByAppendingString:error.reason]; NSLog(@"%@", self.output.text); }
    });
}
- (void)pollBackground:(NSTimer *)timer {
    if (!self.backgroundMonitor.deliveries && self.deadline.timeIntervalSinceNow > 0) return;
    [timer invalidate];
    @try {
        [self check:self.backgroundMonitor.deliveries == 1 && [self.backgroundMonitor.deliveredText.lowercaseString containsString:@"wednesday"] message:@"Deliver a live correction once after background monitor teardown"];
        GKBKeyboardVoiceInput *keyboard = self.backgroundKeyboard;
        [self check:keyboard.resultChanges == 1 && [keyboard.document.text isEqual:self.backgroundMonitor.deliveredText] && !keyboard.observing message:[NSString stringWithFormat:@"Observe one background result through a KVO dictionary and UIKit document update (changes=%lu, inserted=%lu, expected=%lu, observing=%d)", (unsigned long)keyboard.resultChanges, (unsigned long)keyboard.document.text.length, (unsigned long)self.backgroundMonitor.deliveredText.length, keyboard.observing]];
        [GKBPreferences setObjectPreferenceWithValue:@"Ignored later result" forKey:HarnessBackgroundResult];
        [self check:keyboard.resultChanges == 1 && [keyboard.document.text isEqual:self.backgroundMonitor.deliveredText] message:@"Unsubscribe after insertion to prevent duplicate background delivery"];
        [self check:keyboard.finishCalls == 1 && keyboard.fixtureLauncher.completions == 1 && keyboard.fixtureLauncher.delegate.view.subviews.count == 0 message:@"Close the voice UI after a live background result without leaving its overlays attached"];
        [self check:[[self returnGuideStatus] isEqual:@"Your text is ready"] message:@"Keep visible guidance accurate when the background bridge delivers a result"];
        [NSNotificationCenter.defaultCenter postNotificationName:UIApplicationDidEnterBackgroundNotification object:UIApplication.sharedApplication];
        [self runBackgroundAvailabilityChecks];
        NSString *report = DiagnosticReport ? DiagnosticReport() : nil;
        NSDictionary *parsed = report ? [NSJSONSerialization JSONObjectWithData:[report dataUsingEncoding:NSUTF8StringEncoding] options:0 error:nil] : nil;
        [self check:[parsed[@"native_version"] isEqual:@"0.1.8"] && [report containsString:@"monitor_teardown"] && [report containsString:@"speech_request_finished"] && [report containsString:@"keyboard_document_update"] && ![report containsString:self.voice.deliveredText] && ![report containsString:self.backgroundMonitor.deliveredText] message:@"Report the recording/delivery lifecycle without transcript content"];
        for (NSUInteger i = 0; i < 200 && RecordDiagnostic; i++) RecordDiagnostic(@"bounded_trace_test", @{});
        NSDictionary *bounded = DiagnosticReport ? [NSJSONSerialization JSONObjectWithData:[DiagnosticReport() dataUsingEncoding:NSUTF8StringEncoding] options:0 error:nil] : nil;
        [self check:[bounded[@"events"] count] == 180 message:@"Bound retained diagnostic events per process"];
        [VoicePreferences() setBool:NO forKey:@"KiwiRamblerInKeyboard"];
        self.output.text = [NSString stringWithFormat:@"PASS: %lu integration checks\n\n%@\n\n%@", (unsigned long)self.checks.count, [self.checks componentsJoinedByString:@"\n"], self.voice.deliveredText];
        UIButton *screen = [UIButton buttonWithType:UIButtonTypeSystem];
        [screen setTitle:@"Show swipe-back screen" forState:UIControlStateNormal];
        screen.accessibilityIdentifier = @"kiwi.gboard.harness.show-return";
        [screen addTarget:self action:@selector(showReturnScreen:) forControlEvents:UIControlEventTouchUpInside];
        screen.translatesAutoresizingMaskIntoConstraints = NO;
        [self.window addSubview:screen];
        [NSLayoutConstraint activateConstraints:@[
            [screen.leadingAnchor constraintEqualToAnchor:self.window.safeAreaLayoutGuide.leadingAnchor constant:16],
            [screen.bottomAnchor constraintEqualToAnchor:self.window.safeAreaLayoutGuide.bottomAnchor constant:-16]
        ]];
    } @catch (NSException *error) { self.output.text = [@"FAIL: " stringByAppendingString:error.reason]; }
    NSLog(@"%@", self.output.text);
}
- (void)showReturnScreen:(UIButton *)sender {
    [sender removeFromSuperview];
    [VoicePreferences() setBool:YES forKey:@"KiwiRamblerInKeyboard"];
    ((HarnessApplication *)UIApplication.sharedApplication).returnAction = nil;
    self.voice = [self newVoice]; [self.voice startVoiceSearch];
    [[GKBAppNavigation new] navigateBackToPreviousAppWithStartBlock:nil cleanupBlock:nil];
}
- (void)runBackgroundAvailabilityChecks {
    [VoicePreferences() setBool:YES forKey:kGKBVoiceInputAvailableKey];
    [VoicePreferences() setBool:YES forKey:@"HarnessUnrelatedFlag"];
    [self check:![GKBPreferences boolPreferenceForKey:kGKBVoiceInputAvailableKey] message:@"Reject a new background start after the recorder has stopped"];
    [self check:[GKBPreferences boolPreferenceForKey:@"HarnessUnrelatedFlag"] message:@"Preserve unrelated boolean preferences"];
    GKBKeyboardVoiceInput *waiting = [GKBKeyboardVoiceInput new];
    [waiting start];
    [waiting handleAudioLevelChange:@{NSKeyValueChangeNewKey:@0}];
    [self check:waiting.startupTimer.valid message:@"Keep the startup watchdog when the native recorder is not ready"];
    if (PublishCaptureState) PublishCaptureState(YES);
    [waiting handleAppConfirmationChange:[NSDate dateWithTimeIntervalSinceNow:-30]];
    [self check:waiting.startupTimer.valid message:@"Do not acknowledge an unrelated handshake payload"];
    [waiting handleAudioLevelChange:@{NSKeyValueChangeNewKey:@0}];
    [self check:waiting.startupTimer == nil && waiting.lastAudioLevel == 0 message:@"A ready silent recorder clears the watchdog without inventing audio activity"];
    [waiting stopInput];
    if (PublishCaptureState) PublishCaptureState(NO);
    HarnessApplication *app = (HarnessApplication *)UIApplication.sharedApplication;
    app.openedVoiceURL = nil;
    GKBKeyboardVoiceInput *mic = [GKBKeyboardVoiceInput new];
    [mic voiceInputOverlayDidTapMicButton:nil];
    [self check:[app.openedVoiceURL.absoluteString isEqual:@"kiwi-harness://voice/dictate"] && mic.originalMicStarts == 0 message:@"Route the next keyboard-microphone tap through the containing app"];
    if (PublishCaptureState) PublishCaptureState(YES);
    [self check:[GKBPreferences boolPreferenceForKey:kGKBVoiceInputAvailableKey] message:@"Allow keyboard attachment to an already recording host"];
    app.openedVoiceURL = nil; [mic voiceInputOverlayDidTapMicButton:nil];
    [self check:mic.originalMicStarts == 1 && app.openedVoiceURL == nil message:@"Keep the original keyboard path while the host is recording"];
    [VoicePreferences() setObject:@{@"ready":@YES, @"uptime":@(NSProcessInfo.processInfo.systemUptime - 10)} forKey:@"KiwiRamblerCaptureState"];
    [self check:![GKBPreferences boolPreferenceForKey:kGKBVoiceInputAvailableKey] message:@"Expire stale readiness after a host exits or is suspended"];
    [VoicePreferences() setBool:NO forKey:@"KiwiRamblerInKeyboard"];
    [self check:[GKBPreferences boolPreferenceForKey:kGKBVoiceInputAvailableKey] message:@"Preserve original availability with experimental mode disabled"];
    [VoicePreferences() setBool:YES forKey:@"KiwiRamblerInKeyboard"];
    if (PublishCaptureState) PublishCaptureState(NO);
}
- (BOOL)application:(UIApplication *)application didFinishLaunchingWithOptions:(NSDictionary *)options {
    self.checks = [NSMutableArray array];
    return YES;
}
- (void)beginWithWindow:(UIWindow *)window {
    self.window = window;
    UIViewController *controller = [UIViewController new];
    self.output = [[UITextView alloc] initWithFrame:self.window.bounds];
    self.output.accessibilityIdentifier = @"kiwi.gboard.harness.result";
    self.output.editable = NO; self.output.font = [UIFont systemFontOfSize:16];
    self.output.text = @"Testing native Gboard integration…";
    controller.view.backgroundColor = UIColor.systemBackgroundColor;
    self.output.translatesAutoresizingMaskIntoConstraints = NO;
    [controller.view addSubview:self.output];
    [NSLayoutConstraint activateConstraints:@[
        [self.output.topAnchor constraintEqualToAnchor:controller.view.topAnchor],
        [self.output.bottomAnchor constraintEqualToAnchor:controller.view.bottomAnchor],
        [self.output.leadingAnchor constraintEqualToAnchor:controller.view.leadingAnchor],
        [self.output.trailingAnchor constraintEqualToAnchor:controller.view.trailingAnchor]
    ]];
    self.window.rootViewController = controller; [self.window makeKeyAndVisible];
    setenv("KIWI_RAMBLER_TEST_AUDIO", [NSBundle.mainBundle pathForResource:@"KiwiRamblerSample" ofType:@"wav"].UTF8String, 1);
    dispatch_async(dispatch_get_main_queue(), ^{ [self run]; });
}
@end

@interface HarnessSceneDelegate : UIResponder <UIWindowSceneDelegate>
@property(nonatomic, strong) UIWindow *window;
@end
@implementation HarnessSceneDelegate
- (void)scene:(UIScene *)scene willConnectToSession:(UISceneSession *)session options:(UISceneConnectionOptions *)options {
    self.window = [[UIWindow alloc] initWithWindowScene:(UIWindowScene *)scene];
    [(AppDelegate *)UIApplication.sharedApplication.delegate beginWithWindow:self.window];
}
@end

int main(int argc, char **argv) {
    @autoreleasepool { return UIApplicationMain(argc, argv, NSStringFromClass(HarnessApplication.class), NSStringFromClass(AppDelegate.class)); }
}
