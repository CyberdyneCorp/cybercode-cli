/* Test-only Landlock startup boundary. Install before exec/runtime thread
 * creation. An outer bubblewrap mount namespace masks protected subtrees;
 * Landlock alone cannot subtract a deny from an allowed ancestor. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <linux/landlock.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/prctl.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>
#include "linux_initial_exec.h"

#define READ_RIGHTS (LANDLOCK_ACCESS_FS_EXECUTE | LANDLOCK_ACCESS_FS_READ_FILE | LANDLOCK_ACCESS_FS_READ_DIR)
#define WRITE_RIGHTS (LANDLOCK_ACCESS_FS_WRITE_FILE | LANDLOCK_ACCESS_FS_REMOVE_DIR | LANDLOCK_ACCESS_FS_REMOVE_FILE | \
    LANDLOCK_ACCESS_FS_MAKE_DIR | LANDLOCK_ACCESS_FS_MAKE_REG | LANDLOCK_ACCESS_FS_MAKE_SOCK | \
    LANDLOCK_ACCESS_FS_MAKE_FIFO | LANDLOCK_ACCESS_FS_MAKE_SYM | LANDLOCK_ACCESS_FS_REFER | LANDLOCK_ACCESS_FS_TRUNCATE)
#define HANDLED_RIGHTS (READ_RIGHTS | WRITE_RIGHTS | LANDLOCK_ACCESS_FS_MAKE_CHAR | LANDLOCK_ACCESS_FS_MAKE_BLOCK)

static int add_path(int ruleset, const char *path, uint64_t rights) {
    int fd = open(path, O_PATH | O_CLOEXEC);
    struct stat info;
    if (fd < 0 || fstat(fd, &info) != 0) {
        if (fd >= 0) close(fd);
        return -1;
    }
    if (!S_ISDIR(info.st_mode)) {
        rights &= LANDLOCK_ACCESS_FS_EXECUTE | LANDLOCK_ACCESS_FS_READ_FILE |
                  LANDLOCK_ACCESS_FS_WRITE_FILE | LANDLOCK_ACCESS_FS_TRUNCATE;
        if (!S_ISREG(info.st_mode)) rights &= ~LANDLOCK_ACCESS_FS_EXECUTE;
    }
    struct landlock_path_beneath_attr rule = { .allowed_access = rights, .parent_fd = fd };
    int result = syscall(SYS_landlock_add_rule, ruleset, LANDLOCK_RULE_PATH_BENEATH, &rule, 0);
    close(fd);
    return result;
}

static int confine(const char *policy) {
    int abi = syscall(SYS_landlock_create_ruleset, NULL, 0, LANDLOCK_CREATE_RULESET_VERSION);
    if (abi < 3) { fprintf(stderr, "Landlock ABI >= 3 is mandatory (observed %d)\n", abi); return -1; }
    int fd = open(policy, O_RDONLY | O_NOFOLLOW | O_CLOEXEC);
    struct stat info;
    if (fd < 0 || fstat(fd, &info) != 0 || !S_ISREG(info.st_mode) ||
        info.st_nlink != 1 || info.st_size <= 0 || info.st_size > 65536) {
        if (fd >= 0) close(fd);
        return -1;
    }
    size_t size = (size_t)info.st_size, offset = 0;
    char *text = calloc(size + 1, 1);
    if (text == NULL) { close(fd); return -1; }
    while (offset < size) {
        ssize_t count = read(fd, text + offset, size - offset);
        if (count < 0 && errno == EINTR) continue;
        if (count <= 0) { free(text); close(fd); return -1; }
        offset += (size_t)count;
    }
    close(fd);
    if (strlen(text) != size) { free(text); return -1; }
    struct landlock_ruleset_attr attributes = { .handled_access_fs = HANDLED_RIGHTS };
    /* Passing just the fs member size supports ABI 3 without newer struct fields. */
    int ruleset = syscall(SYS_landlock_create_ruleset, &attributes, sizeof(attributes.handled_access_fs), 0);
    if (ruleset < 0) { free(text); return -1; }
    int result = -1;
    char *state = NULL;
    for (char *line = strtok_r(text, "\n", &state); line != NULL; line = strtok_r(NULL, "\n", &state)) {
        if ((line[0] != 'R' && line[0] != 'W') || line[1] != '\t' || line[2] != '/' ||
            strcmp(line + 2, "/") == 0 || strchr(line + 2, '\t') != NULL) goto done;
        if (add_path(ruleset, line + 2, line[0] == 'R' ? READ_RIGHTS : WRITE_RIGHTS) != 0) goto done;
    }
    if (prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0) goto done;
    result = syscall(SYS_landlock_restrict_self, ruleset, 0);
done:
    close(ruleset);
    free(text);
    return result;
}

int main(int argc, char **argv) {
    if (argc < 3) return 2;
    int target = 2, initial = -1;
    if (argc > 3 && (strcmp(argv[2], "--initial-shell=0") == 0 || strcmp(argv[2], "--initial-shell=1") == 0)) {
        initial = strcmp(argv[2], "--initial-shell=1") == 0;
        target = 3;
    }
    if (argv[target][0] != '/') return 2;
    if (confine(argv[1]) != 0) { perror("native Landlock admission failed"); return 2; }
    int limit = getdtablesize();
    if (limit < 3) return 2;
    for (int fd = 3; fd < limit; fd++) close(fd);
    if (initial >= 0 && initial_exec_boundary(initial) != 0) {
        perror("native initial-exec policy admission failed");
        return 2;
    }
    execv(argv[target], argv + target);
    perror("native initial runtime exec failed");
    return 2;
}
