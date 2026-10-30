/* Native Linux startup boundary, promoted from the confinement proof.
 * The caller must supply private mount/PID/network namespaces and protected
 * subtree masks: Landlock cannot subtract a deny from an allowed ancestor. */
#define _GNU_SOURCE
#include <errno.h>
#include <dirent.h>
#include <limits.h>
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
    int fd = open(policy, O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK | O_NOCTTY);
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

/* The listener supervisor forks only from the dedicated single-threaded helper.
 * Fail closed if procfs cannot attest that precondition before Landlock entry. */
static int single_threaded(void) {
    DIR *tasks = opendir("/proc/self/task");
    if (tasks == NULL) return 0;
    unsigned int count = 0;
    struct dirent *entry;
    errno = 0;
    while ((entry = readdir(tasks)) != NULL) {
        if (entry->d_name[0] != '.') count++;
    }
    int valid = errno == 0 && count == 1;
    closedir(tasks);
    return valid;
}

int cyber_plugin_exec(int argc, const char *const argv[]) {
    if (argc < 4 || argv[1][0] != '/' || argv[3][0] != '/') return 2;
    int shell;
    if (strcmp(argv[2], "--initial-shell=0") == 0) shell = 0;
    else if (strcmp(argv[2], "--initial-shell=1") == 0) shell = 1;
    else return 2;
    if (!single_threaded() || confine(argv[1]) != 0) {
        fputs("native Landlock admission failed\n", stderr);
        return 2;
    }
    /* close_range covers descriptors above a lowered RLIMIT_NOFILE too. */
    if (syscall(SYS_close_range, 3U, UINT_MAX, 0) != 0 ||
        initial_exec_boundary(shell) != 0) {
        fputs("native initial-exec policy admission failed\n", stderr);
        return 2;
    }
    /* execv does not modify argv. Rust retains the strings and pointer vector. */
    execv(argv[3], (char *const *)(argv + 3));
    fputs("native initial runtime exec failed\n", stderr);
    return 2;
}
