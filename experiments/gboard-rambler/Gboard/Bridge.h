#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN
NSString * _Nullable KGRGroup(NSString * _Nullable original);
NSUserDefaults *KGRPreferences(void);
NSString *KGRLanguage(id controller);
void KGRShowStatus(id controller, NSString *text);
void KGRDeliverResult(id controller, NSString *text);
void KGRDeliverError(id controller, NSError *error);
NSString * _Nullable KGRLastDictation(void);
void KGRRecordEvent(NSString *event, NSDictionary<NSString *, id> *fields);
NSString *KGRDiagnosticsReport(void);
void KGRClearDiagnostics(void);
void KGRPublishCaptureState(BOOL ready);
NS_ASSUME_NONNULL_END
