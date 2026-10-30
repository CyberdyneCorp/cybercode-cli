/* Controlled post-inspection mutation; the admitted endpoint is private IP. */
#ifdef __linux__
#include <stdatomic.h>

struct mutation_call {
    int fd;
    int result;
    struct sockaddr_storage address;
};
static void *mutation_bind(void *argument) {
    struct mutation_call *call = argument;
    printf("{\"phase\":\"mutation-ready\",\"request_tid\":%ld}\n", syscall(SYS_gettid));
    fflush(stdout);
    /* The audit-only broker pauses after copying this entire address and
     * pinning this descriptor. Its BPIN receipt releases the mutating thread. */
    call->result = bind(call->fd, (struct sockaddr *)&call->address, sizeof(call->address));
    return NULL;
}
static int mutation_probe(void) {
    int parent = syscall(SYS_pidfd_open, getppid(), 0);
    int socket_fd = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (parent < 0 || socket_fd < 0) return 3;
    int fd = fcntl(socket_fd, F_DUPFD_CLOEXEC, 128);
    int kept = fcntl(socket_fd, F_DUPFD_CLOEXEC, 129);
    close(socket_fd);
    if (fd < 0 || kept < 0) return 3;
    struct mutation_call call = { .fd = fd, .result = -1 };
    struct sockaddr_in *ip = (struct sockaddr_in *)&call.address;
    ip->sin_family = AF_INET;
    ip->sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    pthread_t thread;
    if (pthread_create(&thread, NULL, mutation_bind, &call) != 0) return 3;
    if (getchar() != 'R') _exit(3);
    int pair[2];
    if (socketpair(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0, pair) != 0 || dup2(pair[0], fd) != fd) _exit(3);
    close(pair[0]);
    memset(&call.address, 0, sizeof(call.address));
    struct sockaddr_un *unix_address = (struct sockaddr_un *)&call.address;
    unix_address->sun_family = AF_UNIX;
    memcpy(unix_address->sun_path, "mutation-owned", sizeof("mutation-owned"));
    atomic_thread_fence(memory_order_seq_cst);
    /* Signal a pinned parent identity, not a PID that could have been reused. */
    if (syscall(SYS_pidfd_send_signal, parent, SIGUSR2, NULL, 0) != 0 ||
        pthread_join(thread, NULL) != 0) _exit(3);
    struct sockaddr_in observed = {0};
    socklen_t size = sizeof(observed);
    int bound = getsockname(kept, (struct sockaddr *)&observed, &size) == 0 &&
        observed.sin_family == AF_INET && observed.sin_port != 0;
    int domain = 0;
    size = sizeof(domain);
    int replaced = getsockopt(fd, SOL_SOCKET, SO_DOMAIN, &domain, &size) == 0 && domain == AF_UNIX;
    int changed = call.address.ss_family == AF_UNIX;
    printf("{\"phase\":\"mutation-finished\",\"bound_original\":%s,\"fd_replaced\":%s,"
        "\"address_changed\":%s,\"result_ok\":%s}\n", bound ? "true" : "false",
        replaced ? "true" : "false", changed ? "true" : "false", call.result == 0 ? "true" : "false");
    close(parent); close(pair[1]); close(fd); close(kept);
    return bound && replaced && changed && call.result == 0 ? 0 : 3;
}
#else
static int mutation_probe(void) { return 3; }
#endif
