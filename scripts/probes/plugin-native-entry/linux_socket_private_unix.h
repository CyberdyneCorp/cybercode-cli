/* Isolated broker extension. Requires a fresh scratch tree bound as its own
 * mount, with only explicitly owned proxy endpoints initially present, and
 * protected profile/root provenance. No foreign socket may already be linked
 * into it. This is not an owner admission API or complete IPC support. */
#ifndef CYBER_SOCKET_PRIVATE_UNIX_H
#define CYBER_SOCKET_PRIVATE_UNIX_H
#include <linux/openat2.h>
#include <sys/un.h>

static int private_root = -1;
static char private_path[4096];

static int private_initialize(const char *profile) {
    int fd = open(profile, O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK | O_NOCTTY);
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
    int found = 0, valid = strlen(text) == size;
    char *state = NULL;
    for (char *line = strtok_r(text, "\n", &state); valid && line;
         line = strtok_r(NULL, "\n", &state)) {
        if (line[0] != 'U') continue;
        size_t length = strlen(line);
        if (found || length < 4 || line[1] != '\t' || line[2] != '/' ||
            length - 2 >= sizeof(private_path) || line[length - 1] == '/') {
            valid = 0; break;
        }
        for (size_t i = 2; i < length; ++i) {
            unsigned char byte = (unsigned char)line[i];
            if (byte < 32 || byte == 127) valid = 0;
        }
        if (!valid) break;
        strcpy(private_path, line + 2);
        if (strstr(private_path, "//") != NULL) valid = 0;
        /* The owner supplies a canonical root; no lexical . or .. aliases. */
        char copy[sizeof(private_path)];
        strcpy(copy, private_path);
        char *part_state = NULL;
        for (char *part = strtok_r(copy, "/", &part_state); part;
             part = strtok_r(NULL, "/", &part_state)) {
            if (strcmp(part, ".") == 0 || strcmp(part, "..") == 0) valid = 0;
        }
        found = 1;
    }
    free(text);
    if (!valid || !found) return -1;
    private_root = open(private_path, O_PATH | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    return private_root < 0 ? -1 : 0;
}

static int private_make_sock(void) {
    struct landlock_ruleset_attr attributes = {
        .handled_access_fs = LANDLOCK_ACCESS_FS_MAKE_SOCK | LANDLOCK_ACCESS_FS_REFER,
    };
    int ruleset = syscall(SYS_landlock_create_ruleset, &attributes,
                          sizeof(attributes.handled_access_fs), 0);
    if (ruleset < 0) return -1;
    struct landlock_path_beneath_attr rule = {
        .allowed_access = LANDLOCK_ACCESS_FS_MAKE_SOCK, .parent_fd = private_root,
    };
    int result = syscall(SYS_landlock_add_rule, ruleset, LANDLOCK_RULE_PATH_BENEATH, &rule, 0);
    /* REFER is implicitly denied even by a MAKE_SOCK-only layer. Preserve the
     * outer filesystem policy's regular reparenting grants; this layer narrows
     * only socket creation. Separate scratch mounts prevent cross-root links. */
    if (result == 0) result = add_path(ruleset, "/", LANDLOCK_ACCESS_FS_REFER);
    if (result == 0) result = syscall(SYS_landlock_restrict_self, ruleset, 0);
    close(ruleset);
    return result;
}

/* A separate protected scratch bind mount supplies endpoint provenance even
 * when a pathname traverses a symlink or begins from a caller CWD outside it.
 * Both handles remain pinned: mount IDs cannot be recycled out from under them.
 * The owner must prevent foreign links/submounts before admission. */
static int private_socket_mount(int endpoint) {
    struct statx root, socket;
    unsigned int mask = STATX_MNT_ID | STATX_TYPE;
    if (syscall(SYS_statx, private_root, "", AT_EMPTY_PATH, mask, &root) != 0 ||
        syscall(SYS_statx, endpoint, "", AT_EMPTY_PATH, mask, &socket) != 0) return 0;
    return (root.stx_mask & mask) == mask && (socket.stx_mask & mask) == mask &&
        S_ISDIR(root.stx_mode) && S_ISSOCK(socket.stx_mode) &&
        root.stx_mnt_id == socket.stx_mnt_id &&
        root.stx_dev_major == socket.stx_dev_major && root.stx_dev_minor == socket.stx_dev_minor;
}

/* Snapshot the notifying thread's actual directory object, not a guessed path
 * or the broker's CWD. Notification validity is checked again after lookup. */
static int private_context(unsigned int tid) {
    char proc[64];
    snprintf(proc, sizeof(proc), "/proc/%u/cwd", tid);
    return open(proc, O_PATH | O_DIRECTORY | O_CLOEXEC);
}

/* The real socket descriptor was already pinned for the exact notifying thread.
 * Return a pinned pathname endpoint, or -1 for a private abstract/UNSPEC address. */
static int private_endpoint(struct sockaddr_storage *storage, socklen_t length, unsigned int tid, int *endpoint) {
    *endpoint = -1;
    if (storage->ss_family == AF_UNSPEC) return 0;
    if (length <= offsetof(struct sockaddr_un, sun_path) ||
        length > sizeof(struct sockaddr_un) || storage->ss_family != AF_UNIX) return EINVAL;
    struct sockaddr_un *address = (struct sockaddr_un *)storage;
    if (address->sun_path[0] == '\0') return 0; /* Mandatory private net namespace. */
    char path[sizeof(address->sun_path) + 1];
    size_t count = length - offsetof(struct sockaddr_un, sun_path);
    memcpy(path, address->sun_path, count);
    path[count] = '\0';
    int cwd = -1;
    if (path[0] != '/') {
        cwd = private_context(tid);
        if (cwd < 0) return errno;
    }
    /* Ordinary symlinks may resolve to scratch. Magic links are not pathname
     * authority; only the final pinned socket's protected mount grants access. */
    struct open_how how = { .flags = O_PATH | O_CLOEXEC,
        .resolve = RESOLVE_NO_MAGICLINKS };
    int fd = syscall(SYS_openat2, cwd < 0 ? AT_FDCWD : cwd, path, &how, sizeof(how));
    int error = errno;
    if (cwd >= 0) close(cwd);
    if (fd < 0) return error;
    if (!private_socket_mount(fd)) {
        close(fd);
        return EPERM;
    }
    *endpoint = fd;
    return 0;
}
#endif
