/*
 * A test clock for the hub: LD_PRELOAD this and clock_gettime() adds the number of
 * seconds written in $ATLAS_CLOCK_FILE to the wall clock and the monotonic clocks. The
 * file is read on every call, so a running hub can be moved forward in time. Nothing in
 * Atlas is patched; only libc's answer changes.
 */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>

typedef int (*real_t)(clockid_t, struct timespec *);

static long offset(void) {
    const char *path = getenv("ATLAS_CLOCK_FILE");
    long secs = 0;
    if (!path) return 0;
    FILE *f = fopen(path, "r");
    if (!f) return 0;
    if (fscanf(f, "%ld", &secs) != 1) secs = 0;
    fclose(f);
    return secs;
}

int clock_gettime(clockid_t id, struct timespec *ts) {
    static real_t real;
    if (!real) real = (real_t)dlsym(RTLD_NEXT, "clock_gettime");
    int r = real(id, ts);
    if (r == 0 && (id == CLOCK_REALTIME || id == CLOCK_MONOTONIC || id == CLOCK_BOOTTIME))
        ts->tv_sec += offset();
    return r;
}
