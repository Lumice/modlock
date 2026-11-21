#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

/* Pause only the fixture's private import writer, after durable job reservation.
 * Commit hashes in process; the preparation sha256sum child is no longer a
 * writer barrier. LD_PRELOAD is confined to these two test helper processes.
 */
int fsync(int fd) {
    int (*real_fsync)(int) = dlsym(RTLD_NEXT, "fsync");
    const char *root = getenv("LOCAL_TEST_BARRIER");
    char link[64], path[4096], release[4096], marker[4096];
    snprintf(link, sizeof(link), "/proc/self/fd/%d", fd);
    ssize_t n = readlink(link, path, sizeof(path)-1);
    if (root && n > 0) {
        path[n] = 0;
        snprintf(release, sizeof(release), "%s/release", root);
        if (strncmp(path, root, strlen(root)) == 0 &&
            strstr(path, "/.import-local-") && strstr(path, "/owner.json") &&
            access(release, F_OK) != 0) {
            snprintf(marker, sizeof(marker), "%s/reader-pid", root);
            FILE *out = fopen(marker, "w");
            if (!out) _exit(90);
            fprintf(out, "%d", getpid());
            fclose(out);
            while (access(release, F_OK) != 0) usleep(1000);
        }
    }
    return real_fsync(fd);
}
