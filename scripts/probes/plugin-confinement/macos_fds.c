/* Controlled inherited-descriptor fixture. It lowers only this subprocess's
 * soft limit after opening a private descriptor above it. No policy installer. */
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/resource.h>
#include <unistd.h>
int main(int argc, char **argv) {
    if (argc != 4) return 2;
    struct rlimit limit;
    if (getrlimit(RLIMIT_NOFILE, &limit) != 0 || limit.rlim_max <= 500) return 2;
    if (limit.rlim_cur <= 500) {
        limit.rlim_cur = 501;
        if (setrlimit(RLIMIT_NOFILE, &limit) != 0) return 2;
    }
    int source = open(argv[3], O_RDONLY);
    if (source < 0 || dup2(source, 500) < 0 || fcntl(500, F_SETFD, 0) < 0) return 2;
    if (source != 500) close(source);
    limit.rlim_cur = 64;
    if (setrlimit(RLIMIT_NOFILE, &limit) != 0) return 2;
    if (strcmp(argv[1], "-") == 0) {
        execl(argv[2], argv[2], "fd-check", "500", (char *)NULL);
    } else {
        execl(argv[1], argv[1], "-", argv[2], "fd-check", "500", (char *)NULL);
    }
    perror("descriptor fixture exec");
    return 2;
}
