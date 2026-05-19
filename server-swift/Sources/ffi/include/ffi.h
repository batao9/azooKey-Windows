#ifndef ffi_h
#define ffi_h

#include <stdio.h>

#ifdef __cplusplus
extern "C" {
#endif

struct FFICandidate {
    char *text;
    char *subtext;
    char *hiragana;
    int correspondingCount;
};

void FreeCString(char *ptr);
void FreeCandidateList(struct FFICandidate **ptr, int length);

#ifdef __cplusplus
}
#endif

#endif /* ffi_h */
