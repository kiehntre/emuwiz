/* Test-only, process-local journal fault. Never changes the real rename. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int armed;
static int publication_count;

int renameat2(int oldfd, const char *old, int newfd, const char *new,
              unsigned int flags) {
    int (*real)(int, const char *, int, const char *, unsigned int) =
        dlsym(RTLD_NEXT, "renameat2");
    int result = real(oldfd, old, newfd, new, flags);
    const char *published = getenv("REVIEW_CHD_PUBLISHED");
    if (result == 0 && published && strcmp(old, published) == 0) {
        armed = 1;
        fprintf(stderr, "REVIEW_FAULT reverse rename succeeded: %s -> %s\n",
                old, new);
    }
    return result;
}

int renameat(int oldfd, const char *old, int newfd, const char *new) {
    int (*real)(int, const char *, int, const char *) =
        dlsym(RTLD_NEXT, "renameat");
    const char *journal = getenv("REVIEW_OUTPUT_JOURNAL");
    const char *checkpoint = getenv("REVIEW_FAIL_AT");
    if (armed && journal && strcmp(new, journal) == 0 &&
        ++publication_count == (checkpoint ? atoi(checkpoint) : 1)) {
        fprintf(stderr, "REVIEW_FAULT journal publication failed AFTER reverse rename\n");
        errno = EIO;
        return -1;
    }
    return real(oldfd, old, newfd, new);
}
