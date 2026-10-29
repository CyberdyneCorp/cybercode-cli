/* Native private-procfs and proc-root alias receipts. No policy cooperation. */
#include <fcntl.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
static const char *truth(int value) { return value ? "true" : "false"; }
static int readable(const char *path) {
    int fd = open(path, O_RDONLY);
    char byte;
    int result = fd >= 0 && read(fd, &byte, 1) == 1;
    if (fd >= 0) close(fd);
    return result;
}
static int alias(const char *path) {
    char buffer[PATH_MAX];
    int size = snprintf(buffer, sizeof(buffer), "/proc/self/root%s", path);
    return size > 0 && (size_t)size < sizeof(buffer) && readable(buffer);
}
static int private_pid_view(void) {
    FILE *file = fopen("/proc/self/status", "r");
    if (file == NULL) return 0;
    char line[4096];
    int count = 0;
    while (fgets(line, sizeof(line), file) != NULL) {
        if (strncmp(line, "NSpid:", 6) != 0) continue;
        char *current = line + 6;
        while (*current != '\0' && *current != '\n') {
            char *end;
            (void)strtoul(current, &end, 10);
            if (end == current) break;
            count++;
            current = end;
        }
        break;
    }
    fclose(file);
    return count == 1;
}
int main(int argc, char **argv) {
    if (argc != 4) return 2;
    printf("{\"self\":%s,\"init\":%s,\"sys\":%s,\"private_pid_view\":%s,\"project_alias\":%s,\"private_alias\":%s,\"scratch_alias\":%s}\n",
        truth(readable("/proc/self/status")), truth(readable("/proc/1/status")),
        truth(readable("/proc/sys/kernel/pid_max")), truth(private_pid_view()),
        truth(alias(argv[1])), truth(alias(argv[2])), truth(alias(argv[3])));
    return 0;
}
