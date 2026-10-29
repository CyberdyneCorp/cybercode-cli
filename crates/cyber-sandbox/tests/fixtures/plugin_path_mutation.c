/* Fixed native syscall fixture. All paths refer to newly owned test data. */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
int main(int argc, char **argv) {
    if (argc != 4) return 2;
    int result;
    if (strcmp(argv[1], "rename") == 0) result = rename(argv[2], argv[3]);
    else if (strcmp(argv[1], "write") == 0) {
        int fd = open(argv[2], O_WRONLY | O_TRUNC);
        result = fd < 0 ? -1 : (write(fd, "changed-fixture", 15) == 15 ? 0 : -1);
        int saved = errno;
        if (fd >= 0) close(fd);
        errno = saved;
    } else return 2;
    printf("{\"ok\":%s,\"errno\":%d}\n", result == 0 ? "true" : "false", result == 0 ? 0 : errno);
    return 0;
}
