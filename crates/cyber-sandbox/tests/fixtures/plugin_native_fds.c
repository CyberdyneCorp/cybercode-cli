/* Only this controlled process opens a dummy descriptor and lowers its limit. */
#include <fcntl.h>
#include <string.h>
#include <sys/resource.h>
#include <unistd.h>
int main(int argc, char **argv) {
    if (argc != 5) return 2;
    struct rlimit limit;
    if (getrlimit(RLIMIT_NOFILE, &limit) != 0 || limit.rlim_max <= 500) return 2;
    if (limit.rlim_cur <= 500) {
        limit.rlim_cur = 501;
        if (setrlimit(RLIMIT_NOFILE, &limit) != 0) return 2;
    }
    int source = open(argv[4], O_RDONLY);
    if (source < 0 || dup2(source, 500) < 0 || fcntl(500, F_SETFD, 0) < 0) return 2;
    if (source != 500) close(source);
    limit.rlim_cur = 64;
    if (setrlimit(RLIMIT_NOFILE, &limit) != 0) return 2;
    if (strcmp(argv[1], "-") == 0) {
        execl(argv[3], argv[3], "fd-check", "500", (char *)NULL);
    } else {
#ifdef __linux__
        execl(argv[1], argv[1], "--plugin-native-exec", argv[2], "--initial-shell=0", argv[3], "fd-check", "500", (char *)NULL);
#else
        execl(argv[1], argv[1], "--plugin-native-exec", argv[2], argv[3], "fd-check", "500", (char *)NULL);
#endif
    }
    return 2;
}
