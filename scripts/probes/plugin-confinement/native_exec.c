/* Test-only initial exec: inherited non-stdio descriptors never enter Node. */
#include <stdio.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc < 2 || argv[1][0] != '/') return 2;
    int limit = getdtablesize();
    if (limit < 3) return 2;
    for (int fd = 3; fd < limit; fd++) close(fd);
    execv(argv[1], argv + 1);
    perror("native proof exec");
    return 2;
}
