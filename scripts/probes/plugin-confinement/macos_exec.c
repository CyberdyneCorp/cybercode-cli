/* Test-only pre-exec Seatbelt launcher. No initial-exec exception is assumed. */
#include <errno.h>
#include <fcntl.h>
#include <sandbox.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc < 3 || argv[2][0] != '/') return 2;
    if (strcmp(argv[1], "-") != 0) {
        int fd = open(argv[1], O_RDONLY | O_NOFOLLOW | O_CLOEXEC);
        struct stat info;
        if (fd < 0 || fstat(fd, &info) != 0 || !S_ISREG(info.st_mode) ||
            info.st_nlink != 1 || info.st_size <= 0 || info.st_size > 65536) return 2;
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
    int limit = getdtablesize();
    if (limit < 3) return 2;
    for (int fd = 3; fd < limit; fd++) close(fd);
    execv(argv[2], argv + 2);
    int denied = errno == EPERM || errno == EACCES;
    perror("initial native exec");
    return denied ? 13 : 2;
}
