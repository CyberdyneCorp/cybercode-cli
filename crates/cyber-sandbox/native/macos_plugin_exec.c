/* Trusted pre-target Seatbelt entry. Profile and image ownership are caller
 * admission requirements; this function never authorizes a package or generation. */
#include <errno.h>
#include <fcntl.h>
#include <sandbox.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

extern char **environ;

int cyber_plugin_exec(int argc, const char *const argv[]) {
    if (argc < 3 || argv[1][0] != '/' || argv[2][0] != '/') return 2;
    { /* Production entry always installs a policy; no probe bypass. */
        int fd = open(argv[1], O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK | O_NOCTTY);
        struct stat info;
        if (fd < 0 || fstat(fd, &info) != 0 || !S_ISREG(info.st_mode) ||
            info.st_nlink != 1 || info.st_size <= 0 || info.st_size > 65536) {
            if (fd >= 0) close(fd);
            return 2;
        }
        size_t size = (size_t)info.st_size, offset = 0;
        char *profile = calloc(size + 1, 1);
        if (profile == NULL) { close(fd); return 2; }
        while (offset < size) {
            ssize_t count = read(fd, profile + offset, size - offset);
            if (count < 0 && errno == EINTR) continue;
            if (count <= 0) { free(profile); close(fd); return 2; }
            offset += (size_t)count;
        }
        close(fd);
        if (strlen(profile) != size) { free(profile); return 2; }
        char *error = NULL;
        int result = sandbox_init(profile, 0, &error);
        free(profile);
        sandbox_free_error(error);
        if (result != 0) return 2;
    }
    /* Kernel exec-time closure covers descriptors above a lowered soft limit.
     * SETEXEC replaces this process without needing a permitted fork. */
    posix_spawnattr_t attributes;
    posix_spawn_file_actions_t actions;
    if (posix_spawnattr_init(&attributes) != 0) return 2;
    if (posix_spawn_file_actions_init(&actions) != 0) {
        posix_spawnattr_destroy(&attributes);
        return 2;
    }
    int result = posix_spawnattr_setflags(&attributes,
        POSIX_SPAWN_SETEXEC | POSIX_SPAWN_CLOEXEC_DEFAULT);
    for (int fd = STDIN_FILENO; result == 0 && fd <= STDERR_FILENO; fd++) {
        if (fcntl(fd, F_GETFD) >= 0) result = posix_spawn_file_actions_addinherit_np(&actions, fd);
        else if (errno != EBADF) result = errno;
    }
    if (result == 0) result = posix_spawn(NULL, argv[2], &actions, &attributes, (char *const *)(argv + 2), environ);
    posix_spawn_file_actions_destroy(&actions);
    posix_spawnattr_destroy(&attributes);
    errno = result;
    int denied = result == EPERM || result == EACCES;
    perror("initial native exec");
    return denied ? 13 : 2;
}
