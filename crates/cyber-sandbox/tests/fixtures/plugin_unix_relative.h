/* Relative names must resolve from the guest's CWD, never the broker's. */
struct relative_result { int root, parent, escape_connected, escape_sent; };
#ifdef __linux__
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
    int outside = -1;
    if (previous < 0 || root < 0 || fchdir(root) != 0) goto done;
    result.root = local_exchange(AF_UNIX, "relative.sock", -1);
    if (mkdir("relative-dir", 0700) != 0 || chdir("relative-dir") != 0) goto done;
    result.parent = local_exchange(AF_UNIX, "../parent.sock", -1);
    if (fchdir(root) != 0) _exit(3);
    struct sockaddr_un address = { .sun_family = AF_UNIX };
    strcpy(address.sun_path, "../host.sock");
    outside = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
    result.escape_connected = outside >= 0 && connect(outside, (struct sockaddr *)&address,
        (socklen_t)(offsetof(struct sockaddr_un, sun_path) + strlen(address.sun_path) + 1)) == 0;
    result.escape_sent = result.escape_connected && send(outside, "fixture-only", 12, MSG_NOSIGNAL) == 12;
done:
    if (outside >= 0) close(outside);
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
