#ifndef ffi_h
#define ffi_h

#include <stdio.h>

#endif /* ffi_h */

struct FFICandidate {
    char *text;
    char *subtext;
    char *hiragana;
    int correspondingCount;
};

struct FFIClause {
    char *text;
    char *rawHiragana;
    int correspondingCount;
};
