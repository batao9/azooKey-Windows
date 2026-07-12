#ifndef ffi_h
#define ffi_h

#include <stdbool.h>
#include <stdio.h>

#ifdef __cplusplus
extern "C" {
#endif

struct FFICandidate {
    char *text;
    char *subtext;
    char *hiragana;
    int correspondingCount;
    unsigned long long candidateId;
};

void FreeCString(char *ptr);
void FreeCandidateList(struct FFICandidate **ptr, int length);
bool CommitLearningCandidate(unsigned long long candidateId, int commitKind);
bool ResetLearningMemory(void);

#ifdef __cplusplus
}
#endif

#endif /* ffi_h */
