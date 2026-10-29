/* Isolated native experiment only; not compiled into the production helper. */
#ifndef CYBER_UNIX9_POLICY_H
#define CYBER_UNIX9_POLICY_H
/* ABI-9 UAPI value, even when building against an older distro's headers. */
#define CYBER_RESOLVE_UNIX (1ULL << 16)
#define CYBER_ABSTRACT_UNIX 1ULL

static int unix9_add(int ruleset, const char *path, uint64_t rights) {
    int fd = open(path, O_PATH | O_CLOEXEC | O_NOFOLLOW);
    struct stat info;
    if (fd < 0 || fstat(fd, &info) != 0 || !S_ISDIR(info.st_mode)) {
        if (fd >= 0) close(fd);
        return -1;
    }
    struct landlock_path_beneath_attr rule = { .parent_fd = fd, .allowed_access = rights };
    int result = syscall(SYS_landlock_add_rule, ruleset, LANDLOCK_RULE_PATH_BENEATH, &rule, 0);
    close(fd);
    return result;
}

static int confine_unix9(const char *policy) {
    int abi = syscall(SYS_landlock_create_ruleset, NULL, 0, LANDLOCK_CREATE_RULESET_VERSION);
    if (abi < 9) {
        fprintf(stderr, "experimental pathname UNIX policy requires Landlock ABI >= 9 (observed %d)\n", abi);
        return -1;
    }
    int fd = open(policy, O_RDONLY | O_CLOEXEC | O_NOFOLLOW | O_NONBLOCK);
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
    int result = -1, ruleset = -1;
    char *scratch = NULL, *state = NULL;
    if (strlen(text) != size) goto done;
    for (char *line = strtok_r(text, "\n", &state); line != NULL; line = strtok_r(NULL, "\n", &state)) {
        if (line[0] != 'U') continue;
        if (scratch != NULL || line[1] != '\t' || line[2] != '/' ||
            strcmp(line + 2, "/") == 0 || strlen(line + 2) > 4096 ||
            strchr(line + 2, '\t') != NULL) goto done;
        scratch = line + 2;
    }
    if (scratch == NULL) goto done;
    struct { uint64_t fs, net, scoped; } attributes = {
        .fs = CYBER_RESOLVE_UNIX | LANDLOCK_ACCESS_FS_MAKE_SOCK | LANDLOCK_ACCESS_FS_REFER,
        .scoped = CYBER_ABSTRACT_UNIX,
    };
    ruleset = syscall(SYS_landlock_create_ruleset, &attributes, sizeof(attributes), 0);
    if (ruleset < 0 ||
        unix9_add(ruleset, "/", LANDLOCK_ACCESS_FS_REFER) != 0 ||
        unix9_add(ruleset, scratch, attributes.fs) != 0) goto done;
    if (prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0) goto done;
    result = syscall(SYS_landlock_restrict_self, ruleset, 0);
done:
    if (ruleset >= 0) close(ruleset);
    free(text);
    return result;
}
#endif
