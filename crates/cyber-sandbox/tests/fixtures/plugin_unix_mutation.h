/* Real post-pin pathname mutation; all endpoints are owned fixture sockets. */
#ifdef __linux__
#include <stdatomic.h>
struct unix_mutation_call { int fd, result; struct sockaddr_un address; };
static void *unix_mutation_connect(void *argument) {
    struct unix_mutation_call *call = argument;
    printf("{\"phase\":\"mutation-ready\",\"request_tid\":%ld}\n", syscall(SYS_gettid));
    fflush(stdout);
    /* Full sockaddr_un size is the audit fixture's unique pause selector. */
    call->result = connect(call->fd, (struct sockaddr *)&call->address, sizeof(call->address));
    return NULL;
}
static int unix_mutation_probe(const char *host, const char *path) {
    struct sockaddr_un address = { .sun_family = AF_UNIX };
    if (strlen(host) >= sizeof(address.sun_path) || strlen(path) >= sizeof(address.sun_path)) return 3;
    int parent = syscall(SYS_pidfd_open, getppid(), 0);
    int server = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC | SOCK_NONBLOCK, 0);
    int client = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (parent < 0 || server < 0 || client < 0) return 3;
    strcpy(address.sun_path, path);
    if (bind(server, (struct sockaddr *)&address, sizeof(address)) != 0 || listen(server, 1) != 0) return 3;
    int identity = open(path, O_PATH | O_NOFOLLOW | O_CLOEXEC);
    struct stat before, after, replacement;
    if (identity < 0 || fstat(identity, &before) != 0 || !S_ISSOCK(before.st_mode)) return 3;
    int fd = fcntl(client, F_DUPFD_CLOEXEC, 128);
    int kept = fcntl(client, F_DUPFD_CLOEXEC, 129);
    close(client);
    if (fd < 0 || kept < 0) return 3;
    struct unix_mutation_call call = { .fd = fd, .result = -1, .address = address };
    pthread_t thread;
    if (pthread_create(&thread, NULL, unix_mutation_connect, &call) != 0) return 3;
    if (getchar() != 'R') _exit(3);
    int different = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (different < 0 || dup2(different, fd) != fd) _exit(3);
    close(different);
    memset(&call.address, 0, sizeof(call.address));
    call.address.sun_family = AF_UNIX;
    strcpy(call.address.sun_path, host);
    if (unlink(path) != 0 || symlink(host, path) != 0) _exit(3);
    int path_replaced = lstat(path, &replacement) == 0 && S_ISLNK(replacement.st_mode) &&
        (before.st_dev != replacement.st_dev || before.st_ino != replacement.st_ino);
    int original_unlinked = fstat(identity, &after) == 0 && before.st_dev == after.st_dev &&
        before.st_ino == after.st_ino && after.st_nlink == 0;
    atomic_thread_fence(memory_order_seq_cst);
    if (syscall(SYS_pidfd_send_signal, parent, SIGUSR2, NULL, 0) != 0 ||
        pthread_join(thread, NULL) != 0) _exit(3);
    int domain = 0;
    socklen_t length = sizeof(domain);
    int fd_replaced = getsockopt(fd, SOL_SOCKET, SO_DOMAIN, &domain, &length) == 0 && domain == AF_INET;
    int sent = call.result == 0 && send(kept, "fixture-only", 12, MSG_NOSIGNAL) == 12;
    int peer = accept4(server, NULL, NULL, SOCK_CLOEXEC | SOCK_NONBLOCK);
    char token[12], ack = 0;
    int roundtrip = sent && peer >= 0 && recv(peer, token, sizeof(token), MSG_DONTWAIT) == 12 &&
        memcmp(token, "fixture-only", 12) == 0 && send(peer, "A", 1, MSG_NOSIGNAL) == 1 &&
        recv(kept, &ack, 1, MSG_DONTWAIT) == 1 && ack == 'A';
    printf("{\"phase\":\"mutation-finished\",\"fd_replaced\":%s,\"address_changed\":%s,"
        "\"path_replaced\":%s,\"original_unlinked\":%s,\"result_ok\":%s,\"sent\":%s,\"private_roundtrip\":%s}\n",
        fd_replaced ? "true" : "false", strcmp(call.address.sun_path, host) == 0 ? "true" : "false",
        path_replaced ? "true" : "false", original_unlinked ? "true" : "false",
        call.result == 0 ? "true" : "false", sent ? "true" : "false", roundtrip ? "true" : "false");
    if (peer >= 0) close(peer);
    close(parent); close(identity); close(fd); close(kept); close(server); unlink(path);
    /* The owner verifies endpoint selection, including the deliberate host
       delivery counterexample, after all fixture processes/pipes settle. */
    return 0;
}
#else
static int unix_mutation_probe(const char *host, const char *path) {
    (void)host; (void)path; return 3;
}
#endif
