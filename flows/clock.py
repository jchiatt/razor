"""A wall clock the flow sets: Razor's REALTIME reads, and Node's Date.now through gettimeofday,
come from a file; monotonic timers keep real time."""
from contextlib import contextmanager
from pathlib import Path
import subprocess
import tempfile
import time

SOURCE = r"""
#define _GNU_SOURCE
#include <time.h>
#include <sys/time.h>
#include <stdlib.h>
#include <stdio.h>
#include <dlfcn.h>
static long long now(void) {
    const char *path = getenv("PROBE_NOW");
    FILE *file = path ? fopen(path, "r") : NULL;
    long long at = 0;
    if (file) { fscanf(file, "%lld", &at); fclose(file); }
    return at;
}
int clock_gettime(clockid_t id, struct timespec *out) {
    static int (*real_clock)(clockid_t, struct timespec *);
    if (!real_clock) real_clock = dlsym(RTLD_NEXT, "clock_gettime");
    long long at = id == CLOCK_REALTIME ? now() : 0;
    if (!at) return real_clock(id, out);
    out->tv_sec = at / 1000; out->tv_nsec = (at % 1000) * 1000000;
    return 0;
}
int gettimeofday(struct timeval *out, void *zone) {
    static int (*real_time)(struct timeval *, void *);
    if (!real_time) real_time = dlsym(RTLD_NEXT, "gettimeofday");
    long long at = now();
    if (!at) return real_time(out, zone);
    out->tv_sec = at / 1000; out->tv_usec = (at % 1000) * 1000;
    return 0;
}
"""


@contextmanager
def wall_clock():
    """Yields a scratch folder, the environment that sets the clock and `advance(ms)`, which puts
    it at the start plus `ms`; it stands still at the start until then."""
    with tempfile.TemporaryDirectory(prefix="razor-clock-") as scratch:
        folder = Path(scratch)
        (folder / "clock.c").write_text(SOURCE)
        subprocess.run(["cc", "-shared", "-fPIC", str(folder / "clock.c"), "-o", str(folder / "clock.so"), "-ldl"], check=True)
        epoch, wall = int(time.time()) * 1000, folder / "now"

        def advance(ms):
            wall.with_suffix(".next").write_text(str(epoch + ms))
            wall.with_suffix(".next").replace(wall)

        advance(0)
        yield folder, {"LD_PRELOAD": str(folder / "clock.so"), "PROBE_NOW": str(wall)}, advance
