#import <Carbon/Carbon.h>
#import <Foundation/Foundation.h>
#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>
#import <ImageIO/ImageIO.h>
#include <time.h>
#include <unistd.h>
#include <dlfcn.h>
#include <fcntl.h>
#include <sys/file.h>
#include <sys/stat.h>
#include <errno.h>

// A per-user host lease survives caller cancellation while a native worker still
// owns the descriptor. CLOEXEC prevents agents/subprocesses inheriting authority.
int cyber_computer_lease_acquire(void) {
    char path[128]; snprintf(path,sizeof(path),"/private/tmp/cyber-computer-%u.lock",getuid());
    int fd=open(path,O_CREAT|O_RDWR|O_CLOEXEC|O_NOFOLLOW,0600);if(fd<0)return -2;
    struct stat info;
    if(fstat(fd,&info)!=0 || !S_ISREG(info.st_mode) || info.st_uid!=getuid() || info.st_nlink!=1 || fchmod(fd,0600)!=0) {close(fd);return -2;}
    if(flock(fd,LOCK_EX|LOCK_NB)!=0) {int reason=errno;close(fd);return reason==EWOULDBLOCK ? -1 : -2;}
    return fd;
}
void cyber_computer_lease_release(int fd) {if(fd>=0)close(fd);}

// No native request or screenshot is logged. All output has explicit ownership.
void cyber_computer_free(void *pointer) { free(pointer); }
static NSDictionary *decode(const char *text) {
    if (!text) return nil;
    NSData *data = [[NSString stringWithUTF8String:text] dataUsingEncoding:NSUTF8StringEncoding];
    id value = [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
    return [value isKindOfClass:NSDictionary.class] ? value : nil;
}
static int encode(NSDictionary *value, char **output) {
    NSData *data = [NSJSONSerialization dataWithJSONObject:value options:0 error:nil];
    if (!data) return 6;
    char *result = malloc(data.length + 1); if (!result) return 6;
    memcpy(result, data.bytes, data.length); result[data.length] = 0; *output = result; return 0;
}
static uint64_t monotonic_ms(void) {
    struct timespec value; clock_gettime(CLOCK_MONOTONIC, &value);
    return (uint64_t)value.tv_sec * 1000 + (uint64_t)value.tv_nsec / 1000000;
}
static BOOL supported(void) { if (@available(macOS 14.0, *)) return YES; return NO; }
static BOOL protected_app(NSString *bundle) {
    return [@[@"com.apple.systempreferences", @"com.apple.SystemSettings", @"com.apple.SecurityAgent", @"com.apple.loginwindow", @"com.apple.securityagent"] containsObject:bundle ?: @""];
}
static BOOL secure_focus(void) {
    AXUIElementRef system = AXUIElementCreateSystemWide(); AXUIElementSetMessagingTimeout(system,0.25f); CFTypeRef focused = NULL;
    AXError error = AXUIElementCopyAttributeValue(system, kAXFocusedUIElementAttribute, &focused); CFRelease(system);
    if (error != kAXErrorSuccess || !focused) return error != kAXErrorNoValue && error != kAXErrorAttributeUnsupported;
    AXUIElementSetMessagingTimeout((AXUIElementRef)focused,0.25f);
    CFTypeRef role = NULL; CFTypeRef subrole = NULL;
    AXError roleError=AXUIElementCopyAttributeValue((AXUIElementRef)focused, kAXRoleAttribute, &role);
    AXUIElementCopyAttributeValue((AXUIElementRef)focused, kAXSubroleAttribute, &subrole);
    BOOL secure = roleError!=kAXErrorSuccess || (role && CFEqual(role, CFSTR("AXSecureTextField"))) || (subrole && (CFEqual(subrole, CFSTR("AXSecureTextField")) || CFEqual(subrole, CFSTR("AXSystemDialog"))));
    if (role) CFRelease(role); if (subrole) CFRelease(subrole); CFRelease(focused); return secure;
}

int cyber_computer_health(bool prompt, char **output) {
    @autoreleasepool {
        BOOL okay = supported(); BOOL screen = NO; BOOL accessibility = NO;
        if (okay) {
            screen = CGPreflightScreenCaptureAccess();
            if (prompt && !screen) screen = CGRequestScreenCaptureAccess();
            NSDictionary *options = @{ (__bridge NSString *)kAXTrustedCheckOptionPrompt: @(prompt) };
            accessibility = AXIsProcessTrustedWithOptions((__bridge CFDictionaryRef)options);
        }
        return encode(@{ @"supported": @(okay), @"platform": @"macos", @"minimum_version": @"macOS 14", @"screen_capture": @(screen), @"accessibility": @(accessibility), @"binary": @"" }, output);
    }
}

static NSDictionary *window_for_pid(pid_t pid) {
    NSArray *windows = CFBridgingRelease(CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements, kCGNullWindowID));
    for (NSDictionary *window in windows) {
        if ([window[(__bridge NSString *)kCGWindowOwnerPID] intValue] == pid && [window[(__bridge NSString *)kCGWindowLayer] intValue] == 0) return window;
    }
    return nil;
}
static BOOL protected_window(NSDictionary *window) {
    NSString *title = [window[(__bridge NSString *)kCGWindowName] lowercaseString];
    return [title hasPrefix:@"cyber ·"] || [title hasPrefix:@"cyber code"];
}
static NSDictionary *target_with_focus(NSDictionary *selector, BOOL requireFocus) {
    NSRunningApplication *front = NSWorkspace.sharedWorkspace.frontmostApplication;
    if (!front || protected_app(front.bundleIdentifier)) return nil;
    id app = selector[@"app"]; BOOL scoped = [app isKindOfClass:NSString.class];
    if (scoped && ![front.bundleIdentifier isEqualToString:app]) {
        if (requireFocus) return nil;
        NSArray<NSRunningApplication *> *matches = [NSRunningApplication runningApplicationsWithBundleIdentifier:app];
        if (matches.count != 1) return nil;
        front = matches.firstObject;
        if (protected_app(front.bundleIdentifier)) return nil;
    }
    NSDictionary *window = window_for_pid(front.processIdentifier); if (!window || (requireFocus && protected_window(window))) return nil;
    CGRect bounds; CGDirectDisplayID display; uint32_t windowID = [window[(__bridge NSString *)kCGWindowNumber] unsignedIntValue];
    if (scoped) {
        if (!CGRectMakeWithDictionaryRepresentation((__bridge CFDictionaryRef)window[(__bridge NSString *)kCGWindowBounds], &bounds)) return nil;
        uint32_t count = 0; CGDirectDisplayID displays[1];
        if (CGGetDisplaysWithPoint(CGPointMake(CGRectGetMidX(bounds), CGRectGetMidY(bounds)), 1, displays, &count) != kCGErrorSuccess || !count) return nil;
        display = displays[0];
    } else {
        id value = selector[@"display"]; if (![value isKindOfClass:NSNumber.class]) return nil;
        display = [value unsignedIntValue]; if (!display) display = CGMainDisplayID();
        if (!CGDisplayIsActive(display)) return nil; bounds = CGDisplayBounds(display);
    }
    if (bounds.size.width <= 0 || bounds.size.height <= 0) return nil;
    CGFloat scale = 1;
    for (NSScreen *screen in NSScreen.screens) {
        if ([screen.deviceDescription[@"NSScreenNumber"] unsignedIntValue] == display) { scale = screen.backingScaleFactor; break; }
    }
    CGFloat imageWidth = bounds.size.width * scale, imageHeight = bounds.size.height * scale;
    CGFloat ratio = MIN(1.0, 1920.0 / MAX(imageWidth, imageHeight));
    uint32_t width = MAX(1, (uint32_t)floor(imageWidth * ratio)); uint32_t height = MAX(1, (uint32_t)floor(imageHeight * ratio));
    uint64_t launch = (uint64_t)(front.launchDate.timeIntervalSince1970 * 1000.0);
    NSString *identity = scoped ? [NSString stringWithFormat:@"app:%@:%d:%llu:window:%u", app, front.processIdentifier, launch, windowID] : [NSString stringWithFormat:@"display:%u", display];
    return @{ @"identity": identity, @"app": scoped ? app : NSNull.null, @"pid": @(front.processIdentifier), @"window": @(windowID), @"display": @(display), @"launch_ms": @(launch), @"x": @(bounds.origin.x), @"y": @(bounds.origin.y), @"width": @(bounds.size.width), @"height": @(bounds.size.height), @"image_width": @(width), @"image_height": @(height), @"rotation": @((uint16_t)CGDisplayRotation(display)) };
}
static NSDictionary *target(NSDictionary *selector) { return target_with_focus(selector, YES); }
static NSDictionary *selector_for(NSDictionary *value) {
    id app = value[@"app"]; return [app isKindOfClass:NSString.class] ? @{ @"app": app } : @{ @"display": value[@"display"] ?: @0 };
}
static BOOL same_target(NSDictionary *expected, NSDictionary *actual) {
    if (!actual) return NO;
    for (NSString *key in @[@"identity", @"pid", @"launch_ms", @"window", @"display", @"x", @"y", @"width", @"height", @"image_width", @"image_height", @"rotation"]) {
        if (![expected[key] isEqual:actual[key]]) return NO;
    }
    return YES;
}
int cyber_computer_resolve(const char *request, char **output) {
    @autoreleasepool {
        if (!supported()) return 1;
        if (!CGPreflightScreenCaptureAccess()) return 2;
        NSDictionary *value = target_with_focus(decode(request), NO); return value ? encode(value, output) : 3;
    }
}

static BOOL screenkit(void) {
    static void *framework; static dispatch_once_t once;
    dispatch_once(&once, ^{ framework = dlopen("/System/Library/Frameworks/ScreenCaptureKit.framework/ScreenCaptureKit", RTLD_LAZY | RTLD_LOCAL); });
    return framework != NULL;
}
static SCShareableContent *content(void) API_AVAILABLE(macos(14.0));
static SCShareableContent *content(void) {
    dispatch_semaphore_t done = dispatch_semaphore_create(0); __block SCShareableContent *value = nil;
    [NSClassFromString(@"SCShareableContent") getShareableContentExcludingDesktopWindows:YES onScreenWindowsOnly:YES completionHandler:^(SCShareableContent *result, NSError *error) { if (!error) value = result; dispatch_semaphore_signal(done); }];
    if (dispatch_semaphore_wait(done, dispatch_time(DISPATCH_TIME_NOW, 2 * NSEC_PER_SEC)) != 0) return nil;
    return value;
}
int cyber_computer_capture(const char *request, char **metadata, uint8_t **bytes, size_t *length) {
    @autoreleasepool {
        if (@available(macOS 14.0, *)) {
            if (!CGPreflightScreenCaptureAccess()) return 2;
            NSDictionary *expected = decode(request); NSDictionary *actual = target(selector_for(expected));
            if (!same_target(expected, actual)) return 3;
            if (!screenkit()) return 6;
            SCShareableContent *shareable = content(); if (!shareable) return 6;
            SCContentFilter *filter = nil;
            if ([actual[@"app"] isKindOfClass:NSString.class]) {
                for (SCWindow *window in shareable.windows) if (window.windowID == [actual[@"window"] unsignedIntValue]) { filter = [[NSClassFromString(@"SCContentFilter") alloc] initWithDesktopIndependentWindow:window]; break; }
            } else {
                for (SCDisplay *display in shareable.displays) if (display.displayID == [actual[@"display"] unsignedIntValue]) { filter = [[NSClassFromString(@"SCContentFilter") alloc] initWithDisplay:display excludingWindows:@[]]; break; }
            }
            if (!filter) return 3;
            SCStreamConfiguration *configuration = [NSClassFromString(@"SCStreamConfiguration") new]; configuration.width = [actual[@"image_width"] unsignedIntValue]; configuration.height = [actual[@"image_height"] unsignedIntValue]; configuration.showsCursor = NO; configuration.ignoreShadowsSingleWindow = YES;
            dispatch_semaphore_t done = dispatch_semaphore_create(0); __block NSData *png = nil;
            [NSClassFromString(@"SCScreenshotManager") captureImageWithFilter:filter configuration:configuration completionHandler:^(CGImageRef image, NSError *error) {
                if (image && !error) {
                    NSMutableData *data = [NSMutableData data]; CGImageDestinationRef destination = CGImageDestinationCreateWithData((__bridge CFMutableDataRef)data, CFSTR("public.png"), 1, NULL);
                    if (destination) { CGImageDestinationAddImage(destination, image, NULL); if (CGImageDestinationFinalize(destination)) png = data; CFRelease(destination); }
                }
                dispatch_semaphore_signal(done);
            }];
            if (dispatch_semaphore_wait(done, dispatch_time(DISPATCH_TIME_NOW, 3 * NSEC_PER_SEC)) != 0 || !png) return 6;
            if (!CGPreflightScreenCaptureAccess()) return 2;
            if (!same_target(actual, target(selector_for(actual)))) return 3;
            if (png.length > 8 * 1024 * 1024) return 5;
            uint8_t *copy = malloc(png.length); if (!copy) return 6; memcpy(copy, png.bytes, png.length);
            int status = encode(actual, metadata); if (status) { free(copy); return status; }
            *bytes = copy; *length = png.length; return 0;
        }
        return 1;
    }
}

// Public AX window geometry prevents keyboard input entering a different app
// dialog/window merely because its owner PID still matches the captured window.
static BOOL focused_app_window(NSDictionary *expected) {
    if (![expected[@"app"] isKindOfClass:NSString.class]) return YES;
    AXUIElementRef app=AXUIElementCreateApplication([expected[@"pid"] intValue]);AXUIElementSetMessagingTimeout(app,0.25f);
    CFTypeRef window=NULL,position=NULL,size=NULL;
    AXError result=AXUIElementCopyAttributeValue(app,kAXFocusedWindowAttribute,&window);CFRelease(app);
    BOOL okay=NO;
    if(result==kAXErrorSuccess && window && CFGetTypeID(window)==AXUIElementGetTypeID()) {
        AXUIElementSetMessagingTimeout((AXUIElementRef)window,0.25f);
        AXError positioned=AXUIElementCopyAttributeValue((AXUIElementRef)window,kAXPositionAttribute,&position);
        AXError sized=AXUIElementCopyAttributeValue((AXUIElementRef)window,kAXSizeAttribute,&size);
        CGPoint point;CGSize dimensions;
        if(positioned==kAXErrorSuccess && sized==kAXErrorSuccess && position && size && CFGetTypeID(position)==AXValueGetTypeID() && CFGetTypeID(size)==AXValueGetTypeID() && AXValueGetValue(position,kAXValueCGPointType,&point) && AXValueGetValue(size,kAXValueCGSizeType,&dimensions)) {
            okay=fabs(point.x-[expected[@"x"] doubleValue])<1 && fabs(point.y-[expected[@"y"] doubleValue])<1 && fabs(dimensions.width-[expected[@"width"] doubleValue])<1 && fabs(dimensions.height-[expected[@"height"] doubleValue])<1;
        }
    }
    if(position)CFRelease(position);if(size)CFRelease(size);if(window)CFRelease(window);return okay;
}

typedef bool (*Cancel)(const void *);
static int checkpoint(NSDictionary *expected, Cancel cancelled, const void *context, uint64_t deadline) {
    if (cancelled(context)) return 4;
    if (monotonic_ms() >= deadline) return 6;
    if (!AXIsProcessTrusted() || !CGPreflightScreenCaptureAccess()) return 2;
    if (!same_target(expected, target(selector_for(expected))) || !focused_app_window(expected) || secure_focus()) return 3;
    if (cancelled(context)) return 4;
    if (monotonic_ms() >= deadline) return 6;
    return 0;
}
static BOOL visible_point(CGPoint point) {
    uint32_t count=0;CGDirectDisplayID display;
    return isfinite(point.x) && isfinite(point.y) && CGGetDisplaysWithPoint(point,1,&display,&count)==kCGErrorSuccess && count==1;
}
static CGPoint point(NSDictionary *value) { return CGPointMake([value[@"x"] doubleValue], [value[@"y"] doubleValue]); }
static bool mouse(CGEventType type, CGPoint at, CGMouseButton button, int clicks) {
    CGEventRef event = CGEventCreateMouseEvent(NULL, type, at, button); if (!event) return false;
    if (clicks) CGEventSetIntegerValueField(event, kCGMouseEventClickState, clicks);
    CGEventPost(kCGHIDEventTap, event); CFRelease(event); return true;
}
static int click(NSDictionary *expected, NSDictionary *action, Cancel cancel, const void *context, uint64_t deadline) {
    NSString *name = action[@"button"]; CGMouseButton button = [name isEqual:@"right"] ? kCGMouseButtonRight : ([name isEqual:@"middle"] ? kCGMouseButtonCenter : kCGMouseButtonLeft);
    CGEventType down = button == kCGMouseButtonLeft ? kCGEventLeftMouseDown : (button == kCGMouseButtonRight ? kCGEventRightMouseDown : kCGEventOtherMouseDown);
    CGEventType up = button == kCGMouseButtonLeft ? kCGEventLeftMouseUp : (button == kCGMouseButtonRight ? kCGEventRightMouseUp : kCGEventOtherMouseUp);
    for (int n = 1; n <= [action[@"clicks"] intValue]; n++) {
        int status = checkpoint(expected, cancel, context, deadline); if (status) return status;
        CGPoint at = point(action[@"point"]); bool posted = mouse(down, at, button, n); bool released = mouse(up, at, button, n); if (!posted || !released) return 6;
    }
    return 0;
}
static int drag(NSDictionary *expected, NSDictionary *action, Cancel cancel, const void *context, uint64_t deadline) {
    CGPoint from = point(action[@"from"]), to = point(action[@"to"]), current = from;
    uint64_t duration = [action[@"duration_ms"] unsignedLongLongValue], start = monotonic_ms();
    if (!mouse(kCGEventLeftMouseDown, from, kCGMouseButtonLeft, 1)) return 6;
    int status = 0;
    while (monotonic_ms() - start < duration) {
        status = checkpoint(expected, cancel, context, deadline); if (status) break;
        CGFloat fraction = MIN(1.0, (double)(monotonic_ms() - start) / (double)duration);
        CGPoint next = CGPointMake(from.x + (to.x-from.x)*fraction, from.y + (to.y-from.y)*fraction);
        if (!visible_point(next)) { status=3; break; } current=next;
        if (!mouse(kCGEventLeftMouseDragged, current, kCGMouseButtonLeft, 0)) { status = 6; break; } usleep(10000);
    }
    if (!status) { current = to; if (!mouse(kCGEventLeftMouseDragged, to, kCGMouseButtonLeft, 0)) status = 6; }
    // Release is cleanup, even after cancellation; no button remains deliberately held.
    if (!mouse(kCGEventLeftMouseUp, current, kCGMouseButtonLeft, 1)) return 6; return status;
}
static bool keyboard(CGKeyCode key, bool down, CGEventFlags flags, const UniChar *text, UniCharCount length) {
    CGEventRef event = CGEventCreateKeyboardEvent(NULL, key, down); if (!event) return false;
    CGEventSetFlags(event, flags); if (text) CGEventKeyboardSetUnicodeString(event, length, text);
    CGEventPost(kCGHIDEventTap, event); CFRelease(event); return true;
}
static int type_text(NSDictionary *expected, NSString *text, Cancel cancel, const void *context, uint64_t deadline) {
    for (NSUInteger offset = 0; offset < text.length;) {
        int status = checkpoint(expected, cancel, context, deadline); if (status) return status;
        NSUInteger length = MIN((NSUInteger)20, text.length-offset);
        if (offset+length < text.length && CFStringIsSurrogateHighCharacter([text characterAtIndex:offset+length-1])) length--;
        UniChar chunk[20]; [text getCharacters:chunk range:NSMakeRange(offset,length)];
        bool posted = keyboard(0, true, 0, chunk, length); bool released = keyboard(0, false, 0, chunk, length); if (!posted || !released) return 6; offset += length;
    }
    return 0;
}
static int key(NSDictionary *expected, NSDictionary *chord, Cancel cancel, const void *context, uint64_t deadline) {
    static NSDictionary *codes; static dispatch_once_t once;
    dispatch_once(&once, ^{ codes = @{ @"a":@0,@"s":@1,@"d":@2,@"f":@3,@"h":@4,@"g":@5,@"z":@6,@"x":@7,@"c":@8,@"v":@9,@"b":@11,@"q":@12,@"w":@13,@"e":@14,@"r":@15,@"y":@16,@"t":@17,@"1":@18,@"2":@19,@"3":@20,@"4":@21,@"6":@22,@"5":@23,@"9":@25,@"7":@26,@"8":@28,@"0":@29,@"o":@31,@"u":@32,@"i":@34,@"p":@35,@"enter":@36,@"l":@37,@"j":@38,@"k":@40,@"n":@45,@"m":@46,@"tab":@48,@"space":@49,@"backspace":@51,@"escape":@53,@"home":@115,@"page_up":@116,@"delete":@117,@"end":@119,@"page_down":@121,@"left":@123,@"right":@124,@"down":@125,@"up":@126 }; });
    CGEventFlags flags = 0;
    for (NSString *modifier in chord[@"modifiers"]) {
        if ([modifier isEqual:@"shift"]) flags |= kCGEventFlagMaskShift;
        else if ([modifier isEqual:@"control"]) flags |= kCGEventFlagMaskControl;
        else if ([modifier isEqual:@"alt"]) flags |= kCGEventFlagMaskAlternate;
        else if ([modifier isEqual:@"command"]) flags |= kCGEventFlagMaskCommand;
    }
    NSString *name = [chord[@"key"] lowercaseString]; NSNumber *code = codes[name];
    if (name.length == 1 && [@"abcdefghijklmnopqrstuvwxyz0123456789" containsString:name]) {
        // A US physical key map can turn Cmd+A into Cmd+Q on other layouts.
        // Resolve the logical character using the current native layout; fail
        // closed for layouts that cannot produce it without hidden modifiers.
        TISInputSourceRef source = TISCopyCurrentKeyboardLayoutInputSource();
        CFDataRef data = source ? TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData) : NULL;
        code = nil;
        if (data) {
            const UCKeyboardLayout *layout = (const UCKeyboardLayout *)CFDataGetBytePtr(data);
            for (UInt16 candidate = 0; candidate < 128; candidate++) {
                UInt32 dead = 0; UniChar chars[4]; UniCharCount count = 0;
                UInt32 modifiers = ((flags & kCGEventFlagMaskCommand) ? cmdKey : 0) | ((flags & kCGEventFlagMaskShift) ? shiftKey : 0) | ((flags & kCGEventFlagMaskAlternate) ? optionKey : 0) | ((flags & kCGEventFlagMaskControl) ? controlKey : 0);
                OSStatus result = UCKeyTranslate(layout, candidate, kUCKeyActionDown, modifiers >> 8,
                    LMGetKbdType(), kUCKeyTranslateNoDeadKeysBit, &dead, 4, &count, chars);
                if (result == noErr && count == 1 && [[[NSString alloc] initWithCharacters:chars length:1].lowercaseString isEqualToString:name]) { code = @(candidate); break; }
            }
        }
        if (source) CFRelease(source);
    }
    if (!code) return 6;

    // An app grant cannot switch applications or invoke host-wide shortcuts.
    if ([expected[@"app"] isKindOfClass:NSString.class] && (flags & kCGEventFlagMaskControl)) return 3;
    if ([expected[@"app"] isKindOfClass:NSString.class] && flags && ![@[@"a",@"z",@"y",@"s",@"left",@"right",@"up",@"down",@"home",@"end",@"backspace",@"delete"] containsObject:name]) return 3;
    int status = checkpoint(expected, cancel, context, deadline); if (status) return status;
    bool posted = keyboard(code.unsignedShortValue, true, flags, NULL, 0); bool released = keyboard(code.unsignedShortValue, false, 0, NULL, 0); return posted && released ? 0 : 6;
}
int cyber_computer_input(const char *targetJSON, const char *actionJSON, Cancel cancel, const void *context) {
    @autoreleasepool {
        if (!supported()) return 1;
        NSDictionary *expected = decode(targetJSON), *action = decode(actionJSON); if (!expected || !action) return 6;
        uint64_t deadline = monotonic_ms() + 5000; int status = checkpoint(expected, cancel, context, deadline); if (status) return status;
        NSString *kind = action[@"action"];
        if (([kind isEqual:@"move"] || [kind isEqual:@"click"] || [kind isEqual:@"scroll"]) && !visible_point(point(action[@"point"]))) return 3;
        if ([kind isEqual:@"drag"] && (!visible_point(point(action[@"from"])) || !visible_point(point(action[@"to"])))) return 3;
        if ([kind isEqual:@"move"]) { return mouse(kCGEventMouseMoved, point(action[@"point"]), kCGMouseButtonLeft, 0) ? 0 : 6; }
        if ([kind isEqual:@"click"]) return click(expected, action, cancel, context, deadline);
        if ([kind isEqual:@"drag"]) return drag(expected, action, cancel, context, deadline);
        if ([kind isEqual:@"type"]) return type_text(expected, action[@"text"], cancel, context, deadline);
        if ([kind isEqual:@"key"]) return key(expected, action[@"chord"], cancel, context, deadline);
        if ([kind isEqual:@"scroll"]) {
            CGEventRef event = CGEventCreateScrollWheelEvent(NULL, kCGScrollEventUnitPixel, 2, [action[@"dy"] intValue], [action[@"dx"] intValue]);
            if (!event) return 6; CGEventSetLocation(event, point(action[@"point"])); CGEventPost(kCGHIDEventTap, event); CFRelease(event); return 0;
        }
        if ([kind isEqual:@"wait"]) {
            uint64_t until = monotonic_ms() + [action[@"milliseconds"] unsignedLongLongValue];
            while (monotonic_ms() < until) { status = checkpoint(expected, cancel, context, deadline); if (status) return status; usleep(10000); }
            return 0;
        }
        return 6;
    }
}
