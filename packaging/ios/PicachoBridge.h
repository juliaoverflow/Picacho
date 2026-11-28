// What the Swift side calls in the Rust library (src/ios.rs, src/ios/*.rs).
// Strings go as UTF-8 bytes and a length, and are copied before the call
// returns. Completions (`PicachoDone`) are called once, from any thread.
#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef void (*PicachoDone)(void *context, bool ok);

// A notification was tapped (src/ios.rs).
void picacho_ios_notification_tapped(const uint8_t *target, size_t len);
// A notification's button was pressed (src/ios.rs).
void picacho_ios_notification_action(const uint8_t *target, size_t target_len,
                                     const uint8_t *action, size_t action_len,
                                     PicachoDone done, void *context);

// Background app refresh (src/ios.rs).
bool picacho_ios_background_enabled(void);
void picacho_ios_background_refresh(PicachoDone done, void *context);
void picacho_ios_background_expire(void);

// An answer to a request Rust is waiting on (src/ios/requests.rs); a null
// `value` says there is nothing more.
void picacho_ios_answer(int32_t id, const uint8_t *value, size_t len);

// Something for the window to open: `mailto:…`, `attach:<path>`, or a
// message's `picacho://widget/message` link (a Spotlight result, main.m).
void picacho_ios_launch(const uint8_t *request, size_t len);
