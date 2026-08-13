#pragma once
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

enum AsterismAudioMode {
    ASTERISM_AUDIO_NONE = 0,
    ASTERISM_AUDIO_MIC = 1,
    ASTERISM_AUDIO_SYSTEM = 2,
    ASTERISM_AUDIO_BOTH = 3,
};

int asterism_macos_screen_access_ok(void);
int asterism_macos_request_screen_access(void);
int asterism_macos_mic_access_ok(void);
void asterism_macos_request_mic_access(void);

typedef struct AsterismMacRecorder AsterismMacRecorder;

AsterismMacRecorder *asterism_macos_recorder_start(
    const char *output_path,
    int width,
    int height,
    int fps,
    int audio_mode,
    char *err,
    int errlen
);

int asterism_macos_recorder_push_bgra(
    AsterismMacRecorder *rec,
    const uint8_t *bgra,
    int width,
    int height,
    int64_t pts_us,
    char *err,
    int errlen
);

int asterism_macos_recorder_finish(AsterismMacRecorder *rec, char *err, int errlen);

#ifdef __cplusplus
}
#endif
