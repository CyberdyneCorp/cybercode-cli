/* Relative names must resolve from the guest's CWD, never the broker's. */
struct relative_result {
    int root, parent, outside_cwd, alias_relative, alias_absolute, alias_directory;
    int escape_connected, escape_sent, alias_escape_connected, alias_escape_sent;
};
#ifdef __linux__
static int relative_connect_token(const char *path, int *sent) {
    struct sockaddr_un address = { .sun_family = AF_UNIX };
    if (strlen(path) >= sizeof(address.sun_path)) return 0;
    strcpy(address.sun_path, path);
    int socket_fd = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
    int connected = socket_fd >= 0 && connect(socket_fd, (struct sockaddr *)&address,
        (socklen_t)(offsetof(struct sockaddr_un, sun_path) + strlen(path) + 1)) == 0;
    *sent = connected && send(socket_fd, "fixture-only", 12, MSG_NOSIGNAL) == 12;
    if (socket_fd >= 0) close(socket_fd);
    return connected;
}

static int relative_alias_exchange(const char *scratch, const char *name,
                                   const char *value, const char *tail) {
    char target[544], alias[544], destination[576];
    snprintf(target, sizeof(target), "%s/alias-target.sock", scratch);
    snprintf(alias, sizeof(alias), "%s/%s", scratch, name);
    snprintf(destination, sizeof(destination), "%s%s", alias, tail);
    if (symlink(value == NULL ? target : value, alias) != 0) return 0;
    int result = local_exchange_to(AF_UNIX, target, -1, destination);
    unlink(alias);
    return result;
}

static int relative_outside_exchange(const char *scratch, int root) {
    const char *name = strrchr(scratch, '/');
    if (name == NULL) return 0;
    char target[544], destination[544];
    snprintf(target, sizeof(target), "%s/outside-cwd.sock", scratch);
    snprintf(destination, sizeof(destination), "%s/outside-cwd.sock", name + 1);
    int parent = openat(root, "..", O_PATH | O_DIRECTORY | O_CLOEXEC);
    if (parent < 0) return 0;
    int result = fchdir(parent) == 0 && local_exchange_to(AF_UNIX, target, -1, destination);
    if (fchdir(root) != 0) _exit(3);
    close(parent);
    return result;
}

static void relative_aliases(const char *scratch, int root, struct relative_result *result) {
    result->alias_relative = relative_alias_exchange(scratch, "relative-link.sock", "alias-target.sock", "");
    result->alias_absolute = relative_alias_exchange(scratch, "absolute-link.sock", NULL, "");
    result->alias_directory = relative_alias_exchange(scratch, "directory-link", ".", "/alias-target.sock");
    result->outside_cwd = relative_outside_exchange(scratch, root);
    char alias[544];
    snprintf(alias, sizeof(alias), "%s/escape-link.sock", scratch);
    if (symlink("../host.sock", alias) == 0) {
        result->alias_escape_connected = relative_connect_token(alias, &result->alias_escape_sent);
        unlink(alias);
    }
}

static struct relative_result relative_probe(const char *scratch_socket) {
    struct relative_result result = {0};
    if (scratch_socket == NULL || strlen(scratch_socket) >= 512) return result;
    char scratch[512];
    strcpy(scratch, scratch_socket);
    char *last = strrchr(scratch, '/');
    if (last == NULL) return result;
    *last = '\0';
    int previous = open(".", O_PATH | O_DIRECTORY | O_CLOEXEC);
    int root = open(scratch, O_PATH | O_DIRECTORY | O_CLOEXEC);
    if (previous < 0 || root < 0 || fchdir(root) != 0) goto done;
    result.root = local_exchange(AF_UNIX, "relative.sock", -1);
    if (mkdir("relative-dir", 0700) != 0 || chdir("relative-dir") != 0) goto done;
    result.parent = local_exchange(AF_UNIX, "../parent.sock", -1);
    if (fchdir(root) != 0) _exit(3);
    result.escape_connected = relative_connect_token("../host.sock", &result.escape_sent);
    relative_aliases(scratch, root, &result);
done:
    if (root >= 0) { unlinkat(root, "relative-dir", AT_REMOVEDIR); close(root); }
    if (previous >= 0) {
        if (fchdir(previous) != 0) _exit(3);
        close(previous);
    }
    return result;
}
#else
static struct relative_result relative_probe(const char *scratch_socket) {
    (void)scratch_socket; return (struct relative_result){0};
}
#endif
