#import <UIKit/UIKit.h>
#import <objc/message.h>
#import <objc/runtime.h>
#import <dlfcn.h>
#import <math.h>
#import "Bridge.h"
#import "KiwiGboardRambler-Swift.h"

static char SessionKey, ButtonKey, VoiceFlagKey, KeyboardReadyKey;
static char KeyboardGenerationKey, KeyboardUIFinishKey, KeyboardInsertCountKey;
static __weak KiwiGboardRamblerSession *CurrentSession;
// One in-memory recovery result, shown only when the user opens settings.
static NSString *LastDictationText;
NSString *KGRLastDictation(void) { return LastDictationText; }
static NSUUID *NavigationGeneration;
static IMP RecordEndsTime;
// Retain only the finished foreground handoff until the app actually leaves.
// Gboard's keyboard measures its ten-second result window from this timestamp.
static id PendingReturnOwner;
static KiwiGboardRamblerSession *PendingReturnSession;
static NSUUID *PendingReturnGeneration;
static void HideReturnGuide(void);
static void RefreshReturnGuide(void);
static void RequestReturnGuide(KiwiGboardRamblerSession *session, NSUUID *generation);

static void ClearPendingReturn(void) {
    PendingReturnOwner = nil;
    PendingReturnSession = nil;
    PendingReturnGeneration = nil;
}

static void RefreshPendingReturn(void) {
    if (!RecordEndsTime || !PendingReturnOwner || PendingReturnGeneration != NavigationGeneration ||
        PendingReturnSession != CurrentSession || PendingReturnSession.active || !PendingReturnSession.text.length) return;
    ((void (*)(id, SEL))RecordEndsTime)(PendingReturnOwner, NSSelectorFromString(@"recordVoiceInputEndsTime"));
    [KGRPreferences() synchronize];
}

static NSBundle *ContainingBundle(void) {
    NSBundle *bundle = NSBundle.mainBundle;
    if ([bundle.bundlePath.pathExtension isEqualToString:@"appex"]) {
        return [NSBundle bundleWithPath:bundle.bundlePath.stringByDeletingLastPathComponent.stringByDeletingLastPathComponent] ?: bundle;
    }
    return bundle;
}

NSString *KGRGroup(NSString *original) {
    if (!original) return nil;
    id aliases = [NSBundle.mainBundle objectForInfoDictionaryKey:@"KiwiAppGroupAliases"];
    id mapped = [aliases isKindOfClass:NSDictionary.class] ? aliases[original] : nil;
    return [mapped isKindOfClass:NSString.class] ? mapped : original;
}

NSUserDefaults *KGRPreferences(void) {
    return [[NSUserDefaults alloc] initWithSuiteName:KGRGroup(@"group.com.google.keyboard")];
}

// A prepared audio category does not mean a stopped recorder can be restarted
// in the background. Expire this heartbeat if the host exits or is suspended.
void KGRPublishCaptureState(BOOL ready) {
    NSUserDefaults *preferences = KGRPreferences();
    [preferences setObject:@{@"ready":@(ready), @"uptime":@(NSProcessInfo.processInfo.systemUptime)}
        forKey:@"KiwiRamblerCaptureState"];
    [preferences synchronize];
}

static BOOL CaptureIsReady(void) {
    NSUserDefaults *preferences = KGRPreferences();
    [preferences synchronize];
    NSDictionary *state = [preferences dictionaryForKey:@"KiwiRamblerCaptureState"];
    double age = NSProcessInfo.processInfo.systemUptime - [state[@"uptime"] doubleValue];
    return [state[@"ready"] boolValue] && age >= 0 && age < 3;
}

static NSString *VoiceKey(const char *symbol) {
    NSString * __unsafe_unretained *key = (NSString * __unsafe_unretained *)dlsym(RTLD_DEFAULT, symbol);
    return key && [*key isKindOfClass:NSString.class] ? *key : nil;
}

static NSString *DiagnosticRole(void) {
    return [NSBundle.mainBundle.bundlePath.pathExtension isEqual:@"appex"] ? @"keyboard" : @"app";
}

// Separate bounded logs avoid host/extension read-modify-write collisions.
// Callers supply state and counts only, never audio, transcript text or URLs.
void KGRRecordEvent(NSString *event, NSDictionary<NSString *, id> *fields) {
    @synchronized(NSUserDefaults.class) {
        NSUserDefaults *preferences = KGRPreferences();
        NSString *role = DiagnosticRole();
        NSString *key = [@"KiwiRamblerDiagnostics." stringByAppendingString:role];
        NSMutableArray *events = [[preferences arrayForKey:key] mutableCopy] ?: [NSMutableArray array];
        [events addObject:@{@"event":event, @"process":role, @"native_version":@"0.1.8",
            @"uptime":@(NSProcessInfo.processInfo.systemUptime), @"state":fields}];
        if (events.count > 180) [events removeObjectsInRange:NSMakeRange(0, events.count - 180)];
        [preferences setObject:events forKey:key];
        [preferences synchronize];
    }
}

void KGRClearDiagnostics(void) {
    @synchronized(NSUserDefaults.class) {
        NSUserDefaults *preferences = KGRPreferences();
        for (NSString *role in @[@"app", @"keyboard"])
            [preferences removeObjectForKey:[@"KiwiRamblerDiagnostics." stringByAppendingString:role]];
        [preferences synchronize];
    }
}

static id Get(id object, NSString *name) {
    @try { return [object valueForKey:name]; } @catch (NSException *exception) { return nil; }
}

// UI getters in Gboard can lazily create views. Inspect only existing object
// ivars, refusing a different layout/type in an unexpected app version.
static id ObjectIvar(id object, const char *name) {
    Ivar ivar = class_getInstanceVariable(object_getClass(object), name);
    const char *type = ivar ? ivar_getTypeEncoding(ivar) : NULL;
    return type && type[0] == '@' ? object_getIvar(object, ivar) : nil;
}

NSString *KGRLanguage(id controller) {
    id config = Get(controller, @"speechSessionConfig");
    id language = Get(config, @"spokenLanguage");
    NSMutableArray *values = [NSMutableArray array];
    if ([language isKindOfClass:NSString.class]) [values addObject:language];
    for (NSString *key in @[@"BCP47LanguageCode", @"languageCode", @"localeIdentifier", @"code"]) {
        id value = Get(language, key);
        if ([value isKindOfClass:NSString.class]) [values addObject:value];
    }
    [values addObjectsFromArray:NSLocale.preferredLanguages];
    NSRegularExpression *pattern = [NSRegularExpression regularExpressionWithPattern:@"^[A-Za-z]{2,3}([_-][A-Za-z0-9]{2,8})*$" options:0 error:nil];
    for (NSString *value in values) {
        if ([pattern numberOfMatchesInString:value options:0 range:NSMakeRange(0, value.length)])
            return [value stringByReplacingOccurrencesOfString:@"_" withString:@"-"];
    }
    return @"en-US";
}

void KGRShowStatus(id controller, NSString *text) {
    SEL selector = NSSelectorFromString(@"updateTextViewWithStablePrefix:unstableSuffix:shouldAnimate:");
    if ([controller respondsToSelector:selector])
        ((void (*)(id, SEL, id, id, BOOL))objc_msgSend)(controller, selector, @"", text, NO);
    if (objc_getAssociatedObject(controller, &SessionKey) == CurrentSession) RefreshReturnGuide();
}

void KGRDeliverResult(id controller, NSString *text) {
    KGRRecordEvent(@"result_delivery", @{@"characters":@(text.length), @"controller":NSStringFromClass([controller class])});
    LastDictationText = [text copy];
    SEL selector = NSSelectorFromString(@"textStreamingDidRecognizedWithResult:");
    if ([controller respondsToSelector:selector]) ((void (*)(id, SEL, id))objc_msgSend)(controller, selector, text);
    if (objc_getAssociatedObject(controller, &SessionKey) == CurrentSession) RefreshReturnGuide();
}

void KGRDeliverError(id controller, NSError *error) {
    if (objc_getAssociatedObject(controller, &SessionKey) == CurrentSession) HideReturnGuide();
    KGRRecordEvent(@"error_delivery", @{@"domain":error.domain, @"code":@(error.code)});
    SEL selector = NSSelectorFromString(@"textStreamingDidFailWithError:");
    if ([controller respondsToSelector:selector]) ((void (*)(id, SEL, id))objc_msgSend)(controller, selector, error);
}

// Refuse unexpected method ABIs rather than corrupt an updated app's stack.
static BOOL Hook(Class cls, NSString *name, IMP replacement, IMP *original, char returns, unsigned arguments) {
    if (!cls || *original) return NO;
    SEL selector = NSSelectorFromString(name);
    Method method = class_getInstanceMethod(cls, selector);
    if (!method || method_getNumberOfArguments(method) != arguments) return NO;
    char type[64] = {0};
    method_getReturnType(method, type, sizeof(type));
    if (type[0] != returns && !(returns == 'B' && type[0] == 'c')) return NO;
    *original = method_getImplementation(method);
    class_replaceMethod(cls, selector, replacement, method_getTypeEncoding(method));
    return YES;
}

static IMP DefaultsInit, ContainerURL, SuiteName, CommonSuite;
static id Defaults(id self, SEL selector, NSString *name) {
    return ((id (*)(id, SEL, id))DefaultsInit)(self, selector, KGRGroup(name));
}
static id Container(id self, SEL selector, NSString *group) {
    return ((id (*)(id, SEL, id))ContainerURL)(self, selector, KGRGroup(group));
}
static id KeyboardSuite(id self, SEL selector) { return KGRGroup(@"group.com.google.keyboard"); }
static id GoogleSuite(id self, SEL selector) { return KGRGroup(@"group.com.google.common"); }

static KiwiGboardRamblerSession *Session(id controller) {
    id config = Get(controller, @"speechSessionConfig");
    Class voice = NSClassFromString(@"GKBVoiceInputSpeechSessionConfig");
    if (!voice || ![config isKindOfClass:voice]) return nil;
    KiwiGboardRamblerSession *session = objc_getAssociatedObject(controller, &SessionKey);
    if (!session) {
        session = [[KiwiGboardRamblerSession alloc] initWithController:controller];
        objc_setAssociatedObject(controller, &SessionKey, session, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    }
    return session;
}

static id MonitorController(id monitor) {
    Ivar ivar = class_getInstanceVariable(object_getClass(monitor), "_streamingController");
    return ivar ? object_getIvar(monitor, ivar) : nil;
}

// The background monitor polls its original capture source, not the streaming
// controller. Forward that meter to the recorder that actually captures Rambler.
@interface KGRVoiceLevelSource : NSObject
@property(nonatomic, strong) id original;
@property(nonatomic, weak) id monitor;
@end
@implementation KGRVoiceLevelSource
- (double)currentAudioLevel {
    KiwiGboardRamblerSession *session = objc_getAssociatedObject(MonitorController(self.monitor), &SessionKey);
    if (session) return session.level;
    SEL selector = NSSelectorFromString(@"currentAudioLevel");
    return [self.original respondsToSelector:selector]
        ? ((double (*)(id, SEL))objc_msgSend)(self.original, selector) : 0;
}
- (id)forwardingTargetForSelector:(SEL)selector { return self.original; }
- (BOOL)respondsToSelector:(SEL)selector {
    return [super respondsToSelector:selector] || [self.original respondsToSelector:selector];
}
@end

static IMP MonitorLevelStart;
static void StartMonitorLevel(id self, SEL selector, id source) {
    KGRRecordEvent(@"monitor_meter_setup", @{@"source_present":@(source != nil)});
    KGRVoiceLevelSource *bridge = nil;
    if (source) { bridge = [KGRVoiceLevelSource new]; bridge.original = source; bridge.monitor = self; }
    ((void (*)(id, SEL, id))MonitorLevelStart)(self, selector, bridge);
}

static IMP Start, Stop, Finish, Pause, CanStart, Active, Level, Result, EntireResult, MonitorStop;
static void StartVoice(id self, SEL selector) {
    KiwiGboardRamblerSession *session = Session(self);
    KGRRecordEvent(@"voice_start", @{@"native":@(session != nil), @"controller":NSStringFromClass([self class])});
    if (session) {
        HideReturnGuide();
        ClearPendingReturn();
        LastDictationText = nil;
        NavigationGeneration = [NSUUID UUID];
        CurrentSession = session;
        [session start];
    } else ((void (*)(id, SEL))Start)(self, selector);
}
static void FinishVoice(id self, SEL selector) {
    KiwiGboardRamblerSession *session = Session(self);
    KGRRecordEvent(@"voice_finish", @{@"native":@(session != nil), @"cancel":@([[KGRPreferences() stringForKey:@"KiwiRamblerStopAction"] isEqualToString:@"cancel"])});
    if (session) {
        if ([[KGRPreferences() stringForKey:@"KiwiRamblerStopAction"] isEqualToString:@"cancel"]) {
            [session cancel]; HideReturnGuide();
        }
        else [session finish];
    } else ((void (*)(id, SEL))Finish)(self, selector);
}
static void StopVoice(id self, SEL selector) {
    KiwiGboardRamblerSession *session = Session(self);
    if (session) [session finish]; else ((void (*)(id, SEL))Stop)(self, selector);
}
static void PauseVoice(id self, SEL selector) {
    KiwiGboardRamblerSession *session = Session(self);
    KGRRecordEvent(@"voice_pause", @{@"native":@(session != nil)});
    if (session) [session finish]; else ((void (*)(id, SEL))Pause)(self, selector);
}
static BOOL CanStartVoice(id self, SEL selector) {
    return Session(self) ? YES : ((BOOL (*)(id, SEL))CanStart)(self, selector);
}
static BOOL VoiceActive(id self, SEL selector) {
    KiwiGboardRamblerSession *session = Session(self);
    return session ? session.active : ((BOOL (*)(id, SEL))Active)(self, selector);
}
static double AudioLevel(id self, SEL selector) {
    KiwiGboardRamblerSession *session = Session(self);
    return session ? session.level : ((double (*)(id, SEL))Level)(self, selector);
}
static id RecognitionResult(id self, SEL selector) {
    KiwiGboardRamblerSession *session = Session(self);
    return session ? session.text : ((id (*)(id, SEL))Result)(self, selector);
}
static id EntireRecognitionResult(id self, SEL selector) {
    KiwiGboardRamblerSession *session = Session(self);
    return session ? session.text : ((id (*)(id, SEL))EntireResult)(self, selector);
}
static void StopMonitor(id self, SEL selector) {
    // The keyboard's Stop button uses stopSpeechRecognition instead. A monitor
    // shutdown (app closing, audio handoff ending) must discard pending dictation.
    id controller = MonitorController(self);
    KGRRecordEvent(@"monitor_stop", @{@"controller_present":@(controller != nil)});
    [objc_getAssociatedObject(controller, &SessionKey) cancel];
    if (objc_getAssociatedObject(controller, &SessionKey) == CurrentSession) HideReturnGuide();
    ((void (*)(id, SEL))MonitorStop)(self, selector);
}

static IMP MonitorStart, MonitorTearDown, MonitorTrigger, MonitorCancel;
static BOOL StartMonitor(id self, SEL selector) {
    KGRRecordEvent(@"monitor_start", @{});
    BOOL started = ((BOOL (*)(id, SEL))MonitorStart)(self, selector);
    KGRRecordEvent(@"monitor_started", @{@"accepted":@(started), @"controller_present":@(MonitorController(self) != nil)});
    return started;
}
static void TearDownMonitor(id self, SEL selector) {
    KGRRecordEvent(@"monitor_teardown", @{@"controller_present":@(MonitorController(self) != nil)});
    ((void (*)(id, SEL))MonitorTearDown)(self, selector);
}
static void TriggerMonitor(id self, SEL selector, id value) {
    KGRRecordEvent(@"keyboard_trigger_received", @{});
    if ([KGRPreferences() boolForKey:@"KiwiRamblerInKeyboard"] &&
        UIApplication.sharedApplication.applicationState == UIApplicationStateBackground && !CurrentSession.capturing) {
        // A stale availability read must use Gboard's existing foreground retry,
        // rather than ask iOS to activate a new recording from the background.
        NSString *key = VoiceKey("kGKBVoiceInputAppFallbackKey");
        id payload = Get(self, @"triggerDate");
        Class preferences = NSClassFromString(@"GKBPreferences");
        SEL write = NSSelectorFromString(@"setObjectPreferenceWithValue:forKey:");
        if (key && payload && [preferences respondsToSelector:write]) {
            KGRPublishCaptureState(NO);
            KGRRecordEvent(@"background_start_fallback", @{});
            ((void (*)(id, SEL, id, id))objc_msgSend)(preferences, write, payload, key);
            return;
        }
    }
    ((void (*)(id, SEL, id))MonitorTrigger)(self, selector, value);
}
static void CancelMonitor(id self, SEL selector, id value) {
    KGRRecordEvent(@"keyboard_stop_received", @{});
    ((void (*)(id, SEL, id))MonitorCancel)(self, selector, value);
}

static IMP KeyboardClose, KeyboardFinish, KeyboardResult, KeyboardInsert, KeyboardLevel, KeyboardStart, KeyboardStopInput;
static IMP KeyboardConfirmation, KeyboardMic, AvailablePreference, BuildURL, CanOpenURL, OpenURL;
static IMP KeyboardUIFinish;
static BOOL KeyboardUICompletionABI(Class cls) {
    Method method = class_getInstanceMethod(cls, NSSelectorFromString(@"finishWithResult:error:"));
    if (!method || method_getNumberOfArguments(method) != 4) return NO;
    char type[64] = {0};
    method_getReturnType(method, type, sizeof(type));
    if (type[0] != 'v') return NO;
    for (unsigned argument = 2; argument < 4; argument++) {
        method_getArgumentType(method, argument, type, sizeof(type));
        if (type[0] != '@') return NO;
    }
    return YES;
}
static void InvalidateKeyboardUICompletion(id keyboard) {
    objc_setAssociatedObject(keyboard, &KeyboardGenerationKey, [NSUUID UUID], OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    objc_setAssociatedObject(keyboard, &KeyboardUIFinishKey, nil, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
}
static void CompleteKeyboardUI(id self, SEL selector, id result, id error) {
    InvalidateKeyboardUICompletion(self);
    ((void (*)(id, SEL, id, id))KeyboardUIFinish)(self, selector, result, error);
    KGRRecordEvent(@"keyboard_ui_finished", @{@"error":@(error != nil),
        @"overlay_cleared":@(ObjectIvar(self, "_overlay") == nil), @"main_thread":@(NSThread.isMainThread)});
}
static void AcknowledgeKeyboardCapture(id keyboard) {
    SEL response = NSSelectorFromString(@"didReceiveValidResponse");
    if (!objc_getAssociatedObject(keyboard, &KeyboardReadyKey) && [keyboard respondsToSelector:response]) {
        objc_setAssociatedObject(keyboard, &KeyboardReadyKey, @YES, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
        KGRRecordEvent(@"keyboard_capture_acknowledged", @{});
        ((void (*)(id, SEL))objc_msgSend)(keyboard, response);
    }
}
static void ConfirmKeyboard(id self, SEL selector, id payload) {
    BOOL matching = [payload isEqual:Get(self, @"triggerDate")];
    KGRRecordEvent(@"keyboard_confirmation", @{@"matching":@(matching), @"microphone_ready":@(CaptureIsReady())});
    ((void (*)(id, SEL, id))KeyboardConfirmation)(self, selector, payload);
    if (matching && [KGRPreferences() boolForKey:@"KiwiRamblerInKeyboard"] && CaptureIsReady())
        AcknowledgeKeyboardCapture(self);
}
static void StartKeyboard(id self, SEL selector) {
    InvalidateKeyboardUICompletion(self);
    objc_setAssociatedObject(self, &KeyboardReadyKey, nil, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    KGRRecordEvent(@"keyboard_start", @{@"microphone_ready":@(CaptureIsReady())});
    ((void (*)(id, SEL))KeyboardStart)(self, selector);
}
static void StopInputKeyboard(id self, SEL selector) {
    KGRRecordEvent(@"keyboard_stop_input", @{});
    ((void (*)(id, SEL))KeyboardStopInput)(self, selector);
}
static BOOL VoiceAvailablePreference(id self, SEL selector, NSString *key) {
    BOOL original = ((BOOL (*)(id, SEL, id))AvailablePreference)(self, selector, key);
    if (original && [key isEqual:VoiceKey("kGKBVoiceInputAvailableKey")] &&
        [KGRPreferences() boolForKey:@"KiwiRamblerInKeyboard"]) return CaptureIsReady();
    return original;
}
static void LevelInKeyboard(id self, SEL selector, id change) {
    id level = [change isKindOfClass:NSDictionary.class] ? change[NSKeyValueChangeNewKey] : nil;
    if ([KGRPreferences() boolForKey:@"KiwiRamblerInKeyboard"] && [level isKindOfClass:NSNumber.class] &&
        isfinite([level doubleValue]) && CaptureIsReady() && !objc_getAssociatedObject(self, &KeyboardReadyKey)) {
        // Gboard's legacy three-second watchdog treats level > 0.4 as its first
        // response. Quiet speech and silence are valid input from a ready native
        // recorder too; acknowledge that response before its timeout unregisters
        // the result listener. Keep the real meter value for the overlay.
        AcknowledgeKeyboardCapture(self);
    }
    ((void (*)(id, SEL, id))KeyboardLevel)(self, selector, change);
}
static void CloseKeyboard(id self, SEL selector, id overlay) {
    InvalidateKeyboardUICompletion(self);
    KGRRecordEvent(@"keyboard_close", @{});
    [KGRPreferences() setObject:@"cancel" forKey:@"KiwiRamblerStopAction"];
    [KGRPreferences() synchronize];
    ((void (*)(id, SEL, id))KeyboardClose)(self, selector, overlay);
}
static void FinishKeyboard(id self, SEL selector, id overlay) {
    KGRRecordEvent(@"keyboard_stop", @{});
    [KGRPreferences() setObject:@"finish" forKey:@"KiwiRamblerStopAction"];
    [KGRPreferences() synchronize];
    ((void (*)(id, SEL, id))KeyboardFinish)(self, selector, overlay);
}
static void ResultInKeyboard(id self, SEL selector, id value) {
    id text = [value isKindOfClass:NSDictionary.class] ? value[NSKeyValueChangeNewKey] : nil;
    id generation = objc_getAssociatedObject(self, &KeyboardGenerationKey);
    id completion = ObjectIvar(self, "_completion");
    id overlay = ObjectIvar(self, "_overlay");
    NSUInteger insertions = [objc_getAssociatedObject(self, &KeyboardInsertCountKey) unsignedIntegerValue];
    BOOL finishUI = KeyboardUIFinish && generation && completion && [overlay isKindOfClass:UIView.class] &&
        [KGRPreferences() boolForKey:@"KiwiRamblerInKeyboard"] && [text isKindOfClass:NSString.class] &&
        [text length] && ![text isEqual:VoiceKey("kGKBVoiceInputErrorString")];
    KGRRecordEvent(@"keyboard_result_changed", @{@"characters":@([text isKindOfClass:NSString.class] ? [text length] : 0)});
    ((void (*)(id, SEL, id))KeyboardResult)(self, selector, value);
    if (!finishUI || insertions == [objc_getAssociatedObject(self, &KeyboardInsertCountKey) unsignedIntegerValue] ||
        generation != objc_getAssociatedObject(self, &KeyboardGenerationKey) ||
        objc_getAssociatedObject(self, &KeyboardUIFinishKey)) return;

    // handleTextResultChange inserts and calls stopInput, which unregisters
    // listeners but leaves the voice UI and launcher's placeholder attached.
    // Finish through Gboard's normal completion path after that callback unwinds.
    NSUUID *pending = [NSUUID UUID];
    objc_setAssociatedObject(self, &KeyboardUIFinishKey, pending, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    KGRRecordEvent(@"keyboard_ui_finish_scheduled", @{});
    dispatch_async(dispatch_get_main_queue(), ^{
        if (pending != objc_getAssociatedObject(self, &KeyboardUIFinishKey)) return;
        objc_setAssociatedObject(self, &KeyboardUIFinishKey, nil, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
        if (generation != objc_getAssociatedObject(self, &KeyboardGenerationKey) ||
            completion != ObjectIvar(self, "_completion") || overlay != ObjectIvar(self, "_overlay") ||
            ![KGRPreferences() boolForKey:@"KiwiRamblerInKeyboard"] || !KeyboardUICompletionABI(object_getClass(self))) return;
        UIView *parent = [overlay superview];
        ((void (*)(id, SEL, id, id))objc_msgSend)(self, NSSelectorFromString(@"finishWithResult:error:"), text, nil);
        [parent invalidateIntrinsicContentSize];
        [parent setNeedsUpdateConstraints];
        [parent setNeedsLayout];
        [parent layoutIfNeeded];
    });
}
static void InsertInKeyboard(id self, SEL selector, NSString *text) {
    KGRRecordEvent(@"keyboard_document_update", @{@"characters":@(text.length)});
    ((void (*)(id, SEL, id))KeyboardInsert)(self, selector, text);
    if (text.length) {
        NSUInteger count = [objc_getAssociatedObject(self, &KeyboardInsertCountKey) unsignedIntegerValue];
        objc_setAssociatedObject(self, &KeyboardInsertCountKey, @(count + 1), OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    }
}
static NSString *AppScheme(void) {
    for (NSDictionary *entry in [ContainingBundle() objectForInfoDictionaryKey:@"CFBundleURLTypes"]) {
        if ([entry[@"CFBundleURLName"] isEqual:@"kiwi"]) return [entry[@"CFBundleURLSchemes"] firstObject];
    }
    return nil;
}
static NSURL *TranslateURL(NSURL *url, BOOL toGboard) {
    NSString *from = toGboard ? AppScheme() : @"gboard";
    NSString *to = toGboard ? @"gboard" : AppScheme();
    if (!url || !to || ![url.scheme isEqual:from]) return url;
    NSURLComponents *parts = [NSURLComponents componentsWithURL:url resolvingAgainstBaseURL:NO];
    parts.scheme = to;
    return parts.URL ?: url;
}
static void MicInKeyboard(id self, SEL selector, id overlay) {
    InvalidateKeyboardUICompletion(self);
    if ([KGRPreferences() boolForKey:@"KiwiRamblerInKeyboard"] && !CaptureIsReady()) {
        Class urls = NSClassFromString(@"GKBAppURLSchemeUtilities");
        SEL voiceURL = NSSelectorFromString(@"voiceInputURL"), open = NSSelectorFromString(@"gkb_openURL:");
        UIApplication *app = UIApplication.sharedApplication;
        if ([urls respondsToSelector:voiceURL] && [app respondsToSelector:open]) {
            NSURL *url = ((id (*)(id, SEL))objc_msgSend)(urls, voiceURL);
            if (url) {
                KGRRecordEvent(@"keyboard_mic_foreground_restart", @{});
                // This is the same URL/open helper used by Gboard's retry branch.
                ((void (*)(id, SEL, id))objc_msgSend)(app, open, TranslateURL(url, NO));
                return;
            }
        }
    }
    ((void (*)(id, SEL, id))KeyboardMic)(self, selector, overlay);
}
static id GboardAppURL(id self, SEL selector, id host, id path, id query) {
    NSURL *original = ((id (*)(id, SEL, id, id, id))BuildURL)(self, selector, host, path, query);
    return TranslateURL(original, NO);
}
static BOOL GboardCanOpenURL(id self, SEL selector, NSURL *url) {
    return ((BOOL (*)(id, SEL, id))CanOpenURL)(self, selector, TranslateURL(url, YES));
}
static BOOL HostOpenURL(id self, SEL selector, id app, NSURL *url, id options) {
    KGRRecordEvent(@"host_route_received", @{});
    ClearPendingReturn();
    HideReturnGuide();
    return ((BOOL (*)(id, SEL, id, id, id))OpenURL)(self, selector, app, TranslateURL(url, YES), options);
}

// Only this named feature is overridden. Keep Google's flag object and its
// remaining selectors intact, and use the same shared choice in both processes.
@interface KGRVoiceFlag : NSObject
@property(nonatomic, strong) id original;
@end
@implementation KGRVoiceFlag
- (BOOL)value {
    return [KGRPreferences() boolForKey:@"KiwiRamblerInKeyboard"] ||
        ((BOOL (*)(id, SEL))objc_msgSend)(self.original, @selector(value));
}
- (id)forwardingTargetForSelector:(SEL)selector { return self.original; }
- (BOOL)respondsToSelector:(SEL)selector {
    return [super respondsToSelector:selector] || [self.original respondsToSelector:selector];
}
@end

static IMP InKeyboardFlag, Navigate;
static id InKeyboardVoiceFlag(id self, SEL selector) {
    id original = ((id (*)(id, SEL))InKeyboardFlag)(self, selector);
    if (!original || ![KGRPreferences() boolForKey:@"KiwiRamblerInKeyboard"]) return original;
    KGRVoiceFlag *flag = objc_getAssociatedObject(self, &VoiceFlagKey);
    if (!flag) {
        flag = [KGRVoiceFlag new];
        // Associate with the manager, not the wrapped object, to avoid a cycle.
        objc_setAssociatedObject(self, &VoiceFlagKey, flag, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    }
    flag.original = original;
    return flag;
}

static NSSet<UIWindow *> *GboardWindows(void) {
    // This Gboard release predates scenes; keep its legacy window supported.
    NSMutableSet *windows = [NSMutableSet setWithArray:UIApplication.sharedApplication.windows];
    for (UIScene *scene in UIApplication.sharedApplication.connectedScenes) {
        if ([scene isKindOfClass:UIWindowScene.class]) [windows addObjectsFromArray:((UIWindowScene *)scene).windows];
    }
    return windows;
}

// An ordinary child screen, never an alert or sheet. The system's app-switching
// gesture stays available without acknowledging or dismissing anything here.
@interface KGRReturnGuide : UIViewController
@property(nonatomic, strong) KiwiGboardRamblerSession *session;
@property(nonatomic, strong) NSUUID *generation;
@property(nonatomic, strong) UILabel *status, *instructions;
- (void)updateStatus;
@end
@implementation KGRReturnGuide
- (UILabel *)label:(NSString *)text style:(UIFontTextStyle)style identifier:(NSString *)identifier {
    UILabel *label = [UILabel new];
    label.text = text; label.font = [UIFont preferredFontForTextStyle:style];
    label.adjustsFontForContentSizeCategory = YES;
    label.numberOfLines = 0; label.textAlignment = NSTextAlignmentCenter;
    label.accessibilityIdentifier = identifier;
    return label;
}
- (void)viewDidLoad {
    [super viewDidLoad];
    self.view.backgroundColor = UIColor.systemBackgroundColor;
    self.view.accessibilityIdentifier = @"kiwi.rambler.return-guide";
    self.view.accessibilityViewIsModal = YES;
    UIScrollView *scroll = [UIScrollView new];
    scroll.translatesAutoresizingMaskIntoConstraints = NO;
    [self.view addSubview:scroll];
    UIView *content = [UIView new]; content.translatesAutoresizingMaskIntoConstraints = NO;
    [scroll addSubview:content];

    UIView *iconContainer = [UIView new];
    UIImageView *icon = [[UIImageView alloc] initWithImage:[UIImage systemImageNamed:@"arrow.uturn.backward.circle.fill"]];
    icon.translatesAutoresizingMaskIntoConstraints = NO;
    icon.tintColor = UIColor.systemBlueColor; icon.contentMode = UIViewContentModeScaleAspectFit;
    icon.isAccessibilityElement = NO;
    [iconContainer addSubview:icon];
    UILabel *title = [self label:@"Swipe back to your app" style:UIFontTextStyleLargeTitle identifier:@"kiwi.rambler.return-guide.title"];
    title.accessibilityTraits |= UIAccessibilityTraitHeader;
    self.status = [self label:nil style:UIFontTextStyleHeadline identifier:@"kiwi.rambler.return-guide.status"];
    self.status.textColor = UIColor.systemBlueColor;
    self.instructions = [self label:nil style:UIFontTextStyleBody identifier:@"kiwi.rambler.return-guide.instructions"];
    UILabel *gesture = [self label:@"Swipe right along the bottom edge, or use the app switcher." style:UIFontTextStyleCallout identifier:@"kiwi.rambler.return-guide.gesture"];
    gesture.textColor = UIColor.secondaryLabelColor;
    UIStackView *stack = [[UIStackView alloc] initWithArrangedSubviews:@[iconContainer, title, self.status, self.instructions, gesture]];
    stack.axis = UILayoutConstraintAxisVertical; stack.spacing = 20;
    stack.translatesAutoresizingMaskIntoConstraints = NO;
    [stack setCustomSpacing:32 afterView:self.instructions];
    [content addSubview:stack];
    NSLayoutConstraint *fillHeight = [content.heightAnchor constraintEqualToAnchor:scroll.frameLayoutGuide.heightAnchor];
    fillHeight.priority = UILayoutPriorityDefaultLow;
    NSLayoutConstraint *fillWidth = [stack.widthAnchor constraintEqualToAnchor:content.widthAnchor constant:-48];
    fillWidth.priority = UILayoutPriorityDefaultHigh;
    [NSLayoutConstraint activateConstraints:@[
        [scroll.topAnchor constraintEqualToAnchor:self.view.safeAreaLayoutGuide.topAnchor],
        [scroll.bottomAnchor constraintEqualToAnchor:self.view.safeAreaLayoutGuide.bottomAnchor constant:-64],
        [scroll.leadingAnchor constraintEqualToAnchor:self.view.leadingAnchor],
        [scroll.trailingAnchor constraintEqualToAnchor:self.view.trailingAnchor],
        [content.topAnchor constraintEqualToAnchor:scroll.contentLayoutGuide.topAnchor],
        [content.bottomAnchor constraintEqualToAnchor:scroll.contentLayoutGuide.bottomAnchor],
        [content.leadingAnchor constraintEqualToAnchor:scroll.contentLayoutGuide.leadingAnchor],
        [content.trailingAnchor constraintEqualToAnchor:scroll.contentLayoutGuide.trailingAnchor],
        [content.widthAnchor constraintEqualToAnchor:scroll.frameLayoutGuide.widthAnchor],
        [content.heightAnchor constraintGreaterThanOrEqualToAnchor:scroll.frameLayoutGuide.heightAnchor], fillHeight,
        [stack.centerXAnchor constraintEqualToAnchor:content.centerXAnchor],
        [stack.centerYAnchor constraintEqualToAnchor:content.centerYAnchor],
        [stack.topAnchor constraintGreaterThanOrEqualToAnchor:content.topAnchor constant:32],
        [stack.bottomAnchor constraintLessThanOrEqualToAnchor:content.bottomAnchor constant:-32],
        [stack.widthAnchor constraintLessThanOrEqualToConstant:440],
        [stack.widthAnchor constraintLessThanOrEqualToAnchor:content.widthAnchor constant:-48], fillWidth,
        [iconContainer.heightAnchor constraintEqualToConstant:80],
        [icon.centerXAnchor constraintEqualToAnchor:iconContainer.centerXAnchor],
        [icon.centerYAnchor constraintEqualToAnchor:iconContainer.centerYAnchor],
        [icon.heightAnchor constraintEqualToConstant:72], [icon.widthAnchor constraintEqualToConstant:72]
    ]];
    [self updateStatus];
}
- (void)updateStatus {
    if (self.session.capturing) {
        self.status.text = @"Microphone ready";
        self.instructions.text = @"Return to the app you were typing in, then start speaking. Pause to finish or tap Stop on the keyboard.";
    } else if (self.session.active) {
        self.status.text = @"Finishing your dictation…";
        self.instructions.text = @"Return to the app you were typing in. Your text will appear when it is ready.";
    } else {
        self.status.text = @"Your text is ready";
        self.instructions.text = @"Return to the app you were typing in to see your text in the input field.";
    }
}
@end

static KGRReturnGuide *ReturnGuide;
static KiwiGboardRamblerSession *ReturnGuideSession;
static NSUUID *ReturnGuideGeneration;

static void HideReturnGuide(void) {
    ReturnGuideSession = nil; ReturnGuideGeneration = nil;
    KGRReturnGuide *guide = ReturnGuide; ReturnGuide = nil;
    if (!guide) return;
    [guide willMoveToParentViewController:nil];
    [guide.view removeFromSuperview];
    [guide removeFromParentViewController];
    KGRRecordEvent(@"return_guide_hidden", @{});
}

static void RefreshReturnGuide(void) {
    if (!ReturnGuideSession) return;
    if (ReturnGuideGeneration != NavigationGeneration || ReturnGuideSession != CurrentSession ||
        (!ReturnGuideSession.active && !ReturnGuideSession.text.length)) { HideReturnGuide(); return; }
    if (UIApplication.sharedApplication.applicationState != UIApplicationStateActive) return;
    if (ReturnGuide) { [ReturnGuide updateStatus]; return; }
    for (UIWindow *window in GboardWindows()) {
        UIViewController *root = window.rootViewController;
        if (!window.isKeyWindow || window.hidden || !root || window.windowLevel != UIWindowLevelNormal) continue;
        KGRReturnGuide *guide = [KGRReturnGuide new];
        guide.session = ReturnGuideSession; guide.generation = ReturnGuideGeneration;
        [root addChildViewController:guide];
        guide.view.translatesAutoresizingMaskIntoConstraints = NO;
        [root.view addSubview:guide.view];
        [NSLayoutConstraint activateConstraints:@[
            [guide.view.topAnchor constraintEqualToAnchor:root.view.topAnchor],
            [guide.view.bottomAnchor constraintEqualToAnchor:root.view.bottomAnchor],
            [guide.view.leadingAnchor constraintEqualToAnchor:root.view.leadingAnchor],
            [guide.view.trailingAnchor constraintEqualToAnchor:root.view.trailingAnchor]
        ]];
        [guide didMoveToParentViewController:root];
        ReturnGuide = guide;
        KGRRecordEvent(@"return_guide_shown", @{@"capturing":@(guide.session.capturing)});
        break;
    }
}

static void RequestReturnGuide(KiwiGboardRamblerSession *session, NSUUID *generation) {
    if (!session || session != CurrentSession || generation != NavigationGeneration ||
        [NSBundle.mainBundle.bundlePath.pathExtension isEqual:@"appex"] ||
        UIApplication.sharedApplication.applicationState == UIApplicationStateBackground ||
        (!session.microphoneReady && !session.text.length)) return;
    ReturnGuideSession = session; ReturnGuideGeneration = generation;
    // Let Gboard finish dismissing its recognition UI, or let an accepted system
    // return take the app to the background before showing manual guidance.
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 200 * NSEC_PER_MSEC), dispatch_get_main_queue(), ^{
        if (ReturnGuideSession == session && ReturnGuideGeneration == generation) RefreshReturnGuide();
    });
}

// Gboard's 2022 helper returns NO without calling either block when iOS omits
// its return action. Its callers ignore that result, leaving GLIF spinning.
// Preserve its working navigation path, but always complete UI cleanup once.
@interface KGRNavigationAttempt : NSObject
@property(nonatomic, strong) id navigation;
@property(nonatomic, strong) KiwiGboardRamblerSession *session;
@property(nonatomic, strong) NSUUID *generation;
@property(nonatomic, copy) void (^start)(void);
@property(nonatomic, copy) void (^cleanup)(void);
@property(nonatomic) BOOL started, completed, waitsForCapture;
@property(nonatomic, strong) NSDate *deadline;
- (BOOL)run;
@end
@implementation KGRNavigationAttempt
- (void)begin {
    if (self.started || self.completed || NavigationGeneration != self.generation) return;
    self.started = YES;
    KGRRecordEvent(@"handoff_begin", @{@"background_capture":@(self.waitsForCapture), @"microphone_ready":@(self.session.microphoneReady)});
    if (self.start) self.start();
}
- (void)finish {
    if (self.completed) return;
    // This block also records the completion time used by handleVoiceInput's
    // ten-second validity check. It must precede foreground cleanup even when
    // there is no OS navigation. An aborted capture must not be announced ready.
    if (!self.waitsForCapture || self.session.microphoneReady) [self begin];
    self.completed = YES;
    KGRRecordEvent(@"handoff_cleanup", @{@"started":@(self.started)});
    if (self.cleanup) self.cleanup();
    if (self.started) RequestReturnGuide(self.session, self.generation);
    self.cleanup = nil;
    self.start = nil;
}
- (BOOL)run {
    if (NavigationGeneration != self.generation) { [self finish]; return NO; }
    if (self.waitsForCapture && self.session.active && !self.session.microphoneReady && self.deadline.timeIntervalSinceNow > 0) {
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 50 * NSEC_PER_MSEC), dispatch_get_main_queue(), ^{ [self run]; });
        return YES;
    }
    if (self.waitsForCapture && !self.session.microphoneReady) {
        // Do not let a late permission callback start the microphone after an
        // unsuccessful handoff has already dismissed the recognition screen.
        if (self.session.active) [self.session handoffTimedOut];
        [self finish];
        return NO;
    }
    id app = UIApplication.sharedApplication;
    id action = Get(app, @"_systemNavigationAction");
    id destinations = Get(action, @"destinations");
    id destination = [destinations respondsToSelector:@selector(firstObject)] ? [destinations firstObject] : nil;
    BOOL valid = class_getInstanceVariable(object_getClass(app), "_systemNavigationAction") != NULL &&
        [destination respondsToSelector:@selector(unsignedIntValue)] &&
        [action respondsToSelector:NSSelectorFromString(@"sendResponseForDestination:")];
    BOOL pending = [Get(self.navigation, @"hasPendingNavigation") boolValue];
    KGRRecordEvent(@"handoff_route", @{@"available":@(valid), @"pending":@(pending)});
    BOOL accepted = NO;
    if (valid && !pending) {
        accepted = ((BOOL (*)(id, SEL, id, id))Navigate)(self.navigation,
            NSSelectorFromString(@"navigateBackToPreviousAppWithStartBlock:cleanupBlock:"), ^{
                [self begin];
            }, ^{ [self finish]; });
    }
    if (!accepted) dispatch_async(dispatch_get_main_queue(), ^{ [self finish]; });
    // Also handles an accepted request that never calls its cleanup block, or
    // whose private sendResponseForDestination: no longer changes applications.
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, NSEC_PER_SEC), dispatch_get_main_queue(), ^{
        [self finish];
    });
    return accepted;
}
@end

static BOOL NavigateBack(id self, SEL selector, void (^start)(void), void (^cleanup)(void)) {
    KGRNavigationAttempt *attempt = [KGRNavigationAttempt new];
    attempt.navigation = self;
    attempt.session = CurrentSession;
    attempt.generation = NavigationGeneration;
    attempt.start = start;
    attempt.cleanup = cleanup;
    attempt.waitsForCapture = [KGRPreferences() boolForKey:@"KiwiRamblerInKeyboard"] && CurrentSession.active;
    attempt.deadline = [NSDate dateWithTimeIntervalSinceNow:4];
    return [attempt run];
}

static void RecordVoiceEnd(id self, SEL selector) {
    ((void (*)(id, SEL))RecordEndsTime)(self, selector);
    KGRRecordEvent(@"foreground_completion_timestamp", @{});
    NSString *result = Get(self, @"currentResult");
    if (CurrentSession.active || !CurrentSession.text.length || ![result isEqual:CurrentSession.text]) return;
    PendingReturnOwner = self;
    PendingReturnSession = CurrentSession;
    PendingReturnGeneration = NavigationGeneration;
}

static NSDictionary *DiagnosticHooks(void) {
    return @{@"voice_start":@(Start != NULL), @"monitor_start":@(MonitorStart != NULL),
        @"monitor_meter":@(MonitorLevelStart != NULL), @"navigation":@(Navigate != NULL),
        @"keyboard_result":@(KeyboardResult != NULL), @"keyboard_insert":@(KeyboardInsert != NULL),
        @"keyboard_ui_finish":@(KeyboardUIFinish != NULL),
        @"keyboard_level":@(KeyboardLevel != NULL), @"voice_availability":@(AvailablePreference != NULL),
        @"keyboard_mic":@(KeyboardMic != NULL), @"keyboard_confirmation":@(KeyboardConfirmation != NULL)};
}

NSString *KGRDiagnosticsReport(void) {
    NSUserDefaults *preferences = KGRPreferences();
    [preferences synchronize];
    NSMutableArray *events = [NSMutableArray array];
    for (NSString *role in @[@"app", @"keyboard"])
        [events addObjectsFromArray:[preferences arrayForKey:[@"KiwiRamblerDiagnostics." stringByAppendingString:role]] ?: @[]];
    [events sortUsingDescriptors:@[[NSSortDescriptor sortDescriptorWithKey:@"uptime" ascending:YES]]];
    NSDictionary *report = @{
        @"native_version":@"0.1.8", @"kiwi_revision":[ContainingBundle() objectForInfoDictionaryKey:@"KiwiBuildRevision"] ?: @0,
        @"ios":UIDevice.currentDevice.systemVersion, @"process":DiagnosticRole(),
        @"experimental":[NSNumber numberWithBool:[preferences boolForKey:@"KiwiRamblerInKeyboard"]],
        @"hooks":DiagnosticHooks(),
        @"events":events};
    NSData *data = [NSJSONSerialization dataWithJSONObject:report options:NSJSONWritingPrettyPrinted | NSJSONWritingSortedKeys error:nil];
    return data ? [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding] : @"Diagnostic report unavailable.";
}

@interface KGRSettingsAction : NSObject
+ (void)show:(UIButton *)sender;
@end
@implementation KGRSettingsAction
+ (void)show:(UIButton *)sender {
    UIViewController *controller = sender.window.rootViewController;
    while (controller.presentedViewController) controller = controller.presentedViewController;
    [KiwiGboardRamblerSettings presentFrom:controller];
}
@end

static void AddSettingsButton(void) {
    if ([NSBundle.mainBundle.bundlePath.pathExtension isEqual:@"appex"]) return;
    for (UIWindow *window in GboardWindows()) {
            if (!window.isKeyWindow || objc_getAssociatedObject(window, &ButtonKey)) continue;
            UIButton *button = [UIButton buttonWithType:UIButtonTypeSystem];
            button.configuration = [UIButtonConfiguration tintedButtonConfiguration];
            [button setTitle:@"Rambler" forState:UIControlStateNormal];
            button.accessibilityIdentifier = @"kiwi.rambler.settings";
            [button addTarget:KGRSettingsAction.class action:@selector(show:) forControlEvents:UIControlEventTouchUpInside];
            button.translatesAutoresizingMaskIntoConstraints = NO;
            [window addSubview:button];
            [NSLayoutConstraint activateConstraints:@[
                [button.trailingAnchor constraintEqualToAnchor:window.safeAreaLayoutGuide.trailingAnchor constant:-16],
                [button.bottomAnchor constraintEqualToAnchor:window.safeAreaLayoutGuide.bottomAnchor constant:-12]
            ]];
            objc_setAssociatedObject(window, &ButtonKey, button, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    }
}

static void InstallGboardHooks(void) {
    Hook(object_getClass(NSClassFromString(@"GKBSuiteName")), @"suiteName", (IMP)KeyboardSuite, &SuiteName, '@', 2);
    Hook(object_getClass(NSClassFromString(@"GKBSuiteName")), @"googleCommonSuiteName", (IMP)GoogleSuite, &CommonSuite, '@', 2);
    Class controller = NSClassFromString(@"GSKSpeechToTextStreamingController");
    Hook(controller, @"startVoiceSearch", (IMP)StartVoice, &Start, 'v', 2);
    Hook(controller, @"stopVoiceSearch", (IMP)StopVoice, &Stop, 'v', 2);
    Hook(controller, @"stopSpeechRecognition", (IMP)FinishVoice, &Finish, 'v', 2);
    Hook(controller, @"pauseVoiceSearch", (IMP)PauseVoice, &Pause, 'v', 2);
    Hook(controller, @"canPerformVoiceSearch", (IMP)CanStartVoice, &CanStart, 'B', 2);
    Hook(controller, @"isPerformingVoiceSearch", (IMP)VoiceActive, &Active, 'B', 2);
    Hook(controller, @"currentAudioLevel", (IMP)AudioLevel, &Level, 'd', 2);
    Hook(controller, @"recognitionResult", (IMP)RecognitionResult, &Result, '@', 2);
    Hook(controller, @"entireCurrentResult", (IMP)EntireRecognitionResult, &EntireResult, '@', 2);
    Hook(NSClassFromString(@"GKBVoiceInputMonitor"), @"stop", (IMP)StopMonitor, &MonitorStop, 'v', 2);
    Hook(NSClassFromString(@"GKBVoiceInputMonitor"), @"startMonitoringAudioLevelWithSource:", (IMP)StartMonitorLevel, &MonitorLevelStart, 'v', 3);
    Hook(NSClassFromString(@"GKBVoiceInputMonitor"), @"start", (IMP)StartMonitor, &MonitorStart, 'B', 2);
    Hook(NSClassFromString(@"GKBVoiceInputMonitor"), @"tearDown", (IMP)TearDownMonitor, &MonitorTearDown, 'v', 2);
    Hook(NSClassFromString(@"GKBVoiceInputMonitor"), @"handleKeyboardTriggerChange:", (IMP)TriggerMonitor, &MonitorTrigger, 'v', 3);
    Hook(NSClassFromString(@"GKBVoiceInputMonitor"), @"handleKeyboardCancelChange:", (IMP)CancelMonitor, &MonitorCancel, 'v', 3);
    Class keyboard = NSClassFromString(@"GKBKeyboardVoiceInput");
    if (KeyboardUICompletionABI(keyboard))
        Hook(keyboard, @"finishWithResult:error:", (IMP)CompleteKeyboardUI, &KeyboardUIFinish, 'v', 4);
    Hook(keyboard, @"start", (IMP)StartKeyboard, &KeyboardStart, 'v', 2);
    Hook(keyboard, @"handleAppConfirmationChange:", (IMP)ConfirmKeyboard, &KeyboardConfirmation, 'v', 3);
    Hook(keyboard, @"stopInput", (IMP)StopInputKeyboard, &KeyboardStopInput, 'v', 2);
    Hook(keyboard, @"handleAudioLevelChange:", (IMP)LevelInKeyboard, &KeyboardLevel, 'v', 3);
    Hook(keyboard, @"voiceInputOverlayDidTapMicButton:", (IMP)MicInKeyboard, &KeyboardMic, 'v', 3);
    Hook(object_getClass(NSClassFromString(@"GKBPreferences")), @"boolPreferenceForKey:", (IMP)VoiceAvailablePreference, &AvailablePreference, 'B', 3);
    Hook(keyboard, @"voiceInputOverlayDidTapCloseButton:", (IMP)CloseKeyboard, &KeyboardClose, 'v', 3);
    Hook(keyboard, @"voiceInputOverlayDidTapStopButton:", (IMP)FinishKeyboard, &KeyboardFinish, 'v', 3);
    Hook(keyboard, @"handleTextResultChange:", (IMP)ResultInKeyboard, &KeyboardResult, 'v', 3);
    Hook(keyboard, @"updateDocumentProxyWithText:", (IMP)InsertInKeyboard, &KeyboardInsert, 'v', 3);
    Class urls = object_getClass(NSClassFromString(@"GKBAppURLSchemeUtilities"));
    Hook(urls, @"GboardURLWithHost:path:query:", (IMP)GboardAppURL, &BuildURL, '@', 5);
    Hook(urls, @"GboardCanOpenURL:", (IMP)GboardCanOpenURL, &CanOpenURL, 'B', 3);
    Hook(NSClassFromString(@"AppDelegate"), @"application:openURL:options:", (IMP)HostOpenURL, &OpenURL, 'B', 5);
    Hook(NSClassFromString(@"GKBFeatureFlagManager"), @"enableInKeyboardVoiceInput", (IMP)InKeyboardVoiceFlag, &InKeyboardFlag, '@', 2);
    Hook(NSClassFromString(@"GKBAppNavigation"), @"navigateBackToPreviousAppWithStartBlock:cleanupBlock:", (IMP)NavigateBack, &Navigate, 'B', 4);
    Hook(NSClassFromString(@"GKBVoiceRecognitionViewController"), @"recordVoiceInputEndsTime", (IMP)RecordVoiceEnd, &RecordEndsTime, 'v', 2);
}

__attribute__((constructor)) static void LoadKiwiGboardRambler(void) {
    @autoreleasepool {
        // Foundation is already loaded; remap suites even if a Google +load
        // initializer runs before its other Objective-C classes are registered.
        Hook(NSUserDefaults.class, @"initWithSuiteName:", (IMP)Defaults, &DefaultsInit, '@', 3);
        Hook(NSFileManager.class, @"containerURLForSecurityApplicationGroupIdentifier:", (IMP)Container, &ContainerURL, '@', 3);
        InstallGboardHooks();
        dispatch_async(dispatch_get_main_queue(), ^{
            InstallGboardHooks(); AddSettingsButton();
            KGRRecordEvent(@"hooks_ready", DiagnosticHooks());
        });
        [NSNotificationCenter.defaultCenter addObserverForName:UIApplicationDidBecomeActiveNotification object:nil queue:NSOperationQueue.mainQueue usingBlock:^(NSNotification *notification) {
            InstallGboardHooks(); AddSettingsButton();
            RefreshReturnGuide();
        }];
        [NSNotificationCenter.defaultCenter addObserverForName:UIApplicationDidEnterBackgroundNotification object:nil queue:NSOperationQueue.mainQueue usingBlock:^(NSNotification *notification) {
            KGRRecordEvent(@"app_background", @{@"active":@(CurrentSession.active), @"capturing":@(CurrentSession.capturing), @"microphone_ready":@(CurrentSession.microphoneReady)});
            ClearPendingReturn();
            HideReturnGuide();
        }];
        [NSNotificationCenter.defaultCenter addObserverForName:UIApplicationWillResignActiveNotification object:nil queue:NSOperationQueue.mainQueue usingBlock:^(NSNotification *notification) {
            KGRRecordEvent(@"app_resign_active", @{@"active":@(CurrentSession.active), @"capturing":@(CurrentSession.capturing)});
            RefreshPendingReturn();
        }];
    }
}
