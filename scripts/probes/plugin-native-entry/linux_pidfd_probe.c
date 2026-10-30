/* Fixture-only broker prerequisites. No production helper or ambient service. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <poll.h>
#include <pthread.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/prctl.h>
#include <sys/socket.h>
#include <sys/syscall.h>
#include <sys/uio.h>
#include <sys/wait.h>
#include <unistd.h>
#include <sched.h>

/* PIDFD_THREAD is O_EXCL in the kernel UAPI, including older build headers. */
#ifndef PIDFD_THREAD
#define PIDFD_THREAD O_EXCL
#endif

struct notice {
    int process, thread, fd, phase, domain, error;
    uintptr_t address;
    unsigned int length, port;
};
_Static_assert(sizeof(struct notice) <= 64, "bounded fixture frame");
struct context { int command, report, fd; };

static int transfer(int fd, void *bytes, size_t size, int writing) {
    size_t offset = 0;
    while (offset < size) {
        struct pollfd wait = { .fd = fd, .events = writing ? POLLOUT : POLLIN };
        int ready = poll(&wait, 1, 3000);
        if (ready < 0 && errno == EINTR) continue;
        if (ready <= 0) { errno = ready == 0 ? ETIMEDOUT : errno; return -1; }
        ssize_t count = writing ? write(fd, (char *)bytes + offset, size - offset)
                                : read(fd, (char *)bytes + offset, size - offset);
        if (count < 0 && errno == EINTR) continue;
        if (count <= 0) { if (count == 0) errno = EPIPE; return -1; }
        offset += (size_t)count;
    }
    return 0;
}
static int domain(int fd) {
    int result = -1;
    socklen_t size = sizeof(result);
    return getsockopt(fd, SOL_SOCKET, SO_DOMAIN, &result, &size) == 0 ? result : -1;
}
static void report(struct context *c, int phase, volatile struct sockaddr_in *address, int error) {
    struct sockaddr_in bound = {0};
    socklen_t length = sizeof(bound);
    unsigned int port = getsockname(c->fd, (struct sockaddr *)&bound, &length) == 0
                        && bound.sin_family == AF_INET ? ntohs(bound.sin_port) : 0;
    struct notice value = { .process = getpid(), .thread = (int)syscall(SYS_gettid),
        .fd = c->fd, .phase = phase, .domain = domain(c->fd), .error = error,
        .address = (uintptr_t)address, .length = sizeof(*address), .port = port };
    if (transfer(c->report, &value, sizeof(value), 1) != 0) _exit(3);
}
static void expect(struct context *c, char expected) {
    char actual;
    if (transfer(c->command, &actual, 1, 0) != 0 || actual != expected) _exit(3);
}
static void *worker(void *argument) {
    struct context *c = argument;
    volatile struct sockaddr_in address = { .sin_family = AF_INET,
        .sin_addr.s_addr = htonl(INADDR_LOOPBACK) };
    if (unshare(CLONE_FILES) != 0) { report(c, -1, &address, errno); return NULL; }
    int fd = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (fd < 0 || dup2(fd, c->fd) != c->fd) _exit(3);
    if (fd != c->fd) close(fd);
    report(c, 0, &address, 0);
    expect(c, 'M');
    volatile unsigned char *bytes = (volatile unsigned char *)&address;
    for (size_t i = 0; i < sizeof(address); i++) bytes[i] = 0xa5;
    report(c, 1, &address, 0);
    expect(c, 'B');
    report(c, 2, &address, 0);
    expect(c, 'C');
    int pair[2];
    if (socketpair(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0, pair) != 0 ||
        dup2(pair[0], c->fd) != c->fd) _exit(3);
    if (pair[0] != c->fd) close(pair[0]);
    close(pair[1]);
    report(c, 3, &address, 0);
    expect(c, 'X');
    return NULL;
}
static void child(struct context c) {
    int pair[2];
    if (socketpair(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0, pair) != 0) _exit(3);
    c.fd = pair[0]; close(pair[1]);
    pthread_t thread;
    if (pthread_create(&thread, NULL, worker, &c) != 0 || pthread_join(thread, NULL) != 0) _exit(3);
    struct sockaddr_in cell = {0};
    report(&c, 4, &cell, 0);
    expect(&c, 'D');
    if (prctl(PR_SET_DUMPABLE, 0, 0, 0, 0) != 0) _exit(3);
    report(&c, 5, &cell, 0);
    expect(&c, 'Q');
    _exit(0);
}
static ssize_t copy_memory(pid_t pid, uintptr_t address, void *out, size_t size) {
    struct iovec local = { .iov_base = out, .iov_len = size };
    struct iovec remote = { .iov_base = (void *)address, .iov_len = size };
    ssize_t result = process_vm_readv(pid, &local, 1, &remote, 1, 0);
    if (result >= 0 && result != (ssize_t)size) errno = EIO;
    return result;
}
static int roundtrip(int listener, const struct sockaddr_in *address) {
    int client = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0), peer = -1, ok = 0;
    if (client < 0 || connect(client, (const struct sockaddr *)address, sizeof(*address)) != 0) goto done;
    peer = accept4(listener, NULL, NULL, SOCK_CLOEXEC);
    char token[12];
    ok = peer >= 0 && send(client, "fixture-only", 12, MSG_NOSIGNAL) == 12 &&
        recv(peer, token, sizeof(token), MSG_WAITALL) == 12 && memcmp(token, "fixture-only", 12) == 0;
done:
    if (peer >= 0) close(peer);
    if (client >= 0) close(client);
    return ok;
}

int main(void) {
    int commands[2], reports[2];
    if (pipe2(commands, O_CLOEXEC) != 0 || pipe2(reports, O_CLOEXEC) != 0) return 2;
    signal(SIGPIPE, SIG_IGN);
    pid_t pid = fork();
    if (pid < 0) return 2;
    if (pid == 0) {
        close(commands[1]); close(reports[0]);
        child((struct context){ .command = commands[0], .report = reports[1] });
    }
    close(commands[0]); close(reports[1]);
    const char *stage = "initial_notice", *outcome = "failed";
    int error = 0, process_handle = -1, thread_handle = -1, wrong = -1, pinned = -1, replacement = -1;
    int leader_domain = -1, thread_domain = -1, replacement_domain = -1, pinned_domain = -1;
    int copied_changed = 0, guest_bound = 0, tcp = 0, thread_gone = 0, private_fd = 0, private_memory = 0;
    struct notice n;
    struct sockaddr_in snapshot, changed, bound;
    socklen_t length = sizeof(bound);
#define CHECK(test, label) do { stage = label; if (!(test)) { error = errno; goto done; } } while (0)
#define CAP(test, label) do { stage = label; if (!(test)) { error = errno; \
    if (error == EPERM || error == ENOSYS || error == EINVAL) { outcome = "unavailable"; } \
    goto done; } } while (0)
#define COMMAND(value) do { char byte = value; CHECK(transfer(commands[1], &byte, 1, 1) == 0, "command"); } while (0)
#define NOTICE(phase_value) do { CHECK(transfer(reports[0], &n, sizeof(n), 0) == 0, "notice"); \
    CHECK(n.phase == phase_value, "phase"); } while (0)
    CHECK(transfer(reports[0], &n, sizeof(n), 0) == 0, "initial_notice");
    if (n.phase == -1) {
        stage = "unshare_files"; error = n.error;
        if (error == EPERM || error == ENOSYS || error == EINVAL) outcome = "unavailable";
        goto done;
    }
    CHECK(n.phase == 0 && n.process == pid && n.thread != pid && n.length == sizeof(snapshot), "initial_identity");
    CAP((process_handle = (int)syscall(SYS_pidfd_open, pid, 0)) >= 0, "pidfd_process");
    CAP((thread_handle = (int)syscall(SYS_pidfd_open, n.thread, PIDFD_THREAD)) >= 0, "pidfd_thread");
    CAP((wrong = (int)syscall(SYS_pidfd_getfd, process_handle, n.fd, 0)) >= 0, "duplicate_process");
    CAP((pinned = (int)syscall(SYS_pidfd_getfd, thread_handle, n.fd, 0)) >= 0, "duplicate_thread");
    leader_domain = domain(wrong); thread_domain = domain(pinned);
    CHECK(leader_domain == AF_UNIX && thread_domain == AF_INET, "thread_file_table");
    CAP(copy_memory(n.thread, n.address, &snapshot, sizeof(snapshot)) == (ssize_t)sizeof(snapshot), "copy_guest_memory");
    CHECK(snapshot.sin_family == AF_INET && snapshot.sin_addr.s_addr == htonl(INADDR_LOOPBACK) && snapshot.sin_port == 0, "copied_address");
    COMMAND('M'); NOTICE(1);
    CHECK(copy_memory(n.thread, n.address, &changed, sizeof(changed)) == (ssize_t)sizeof(changed), "copy_changed_memory");
    copied_changed = memcmp(&snapshot, &changed, sizeof(snapshot)) != 0;
    CHECK(copied_changed, "guest_changed_address");
    CHECK(bind(pinned, (struct sockaddr *)&snapshot, sizeof(snapshot)) == 0 && listen(pinned, 1) == 0, "pinned_bind_listen");
    CHECK(getsockname(pinned, (struct sockaddr *)&bound, &length) == 0, "bound_address");
    COMMAND('B'); NOTICE(2);
    guest_bound = n.port == ntohs(bound.sin_port) && n.port != 0;
    CHECK(guest_bound, "guest_observed_bind");
    COMMAND('C'); NOTICE(3);
    CAP((replacement = (int)syscall(SYS_pidfd_getfd, thread_handle, n.fd, 0)) >= 0, "duplicate_replacement");
    replacement_domain = domain(replacement); pinned_domain = domain(pinned);
    CHECK(replacement_domain == AF_UNIX && pinned_domain == AF_INET, "descriptor_replacement");
    tcp = roundtrip(pinned, &bound);
    CHECK(tcp, "pinned_tcp_roundtrip");
    COMMAND('X'); NOTICE(4);
    int dead = (int)syscall(SYS_pidfd_getfd, thread_handle, n.fd, 0);
    thread_gone = dead < 0 ? errno : 0; if (dead >= 0) close(dead);
    CHECK(thread_gone == ESRCH, "thread_handle_after_exit");
    COMMAND('D'); NOTICE(5);
    int denied = (int)syscall(SYS_pidfd_getfd, process_handle, n.fd, 0);
    private_fd = denied < 0 ? errno : 0; if (denied >= 0) close(denied);
    private_memory = copy_memory(pid, n.address, &changed, sizeof(changed)) < 0 ? errno : 0;
    CHECK(private_fd == EPERM && private_memory == EPERM, "nondumpable_denial");
    COMMAND('Q');
    outcome = "passed"; stage = "complete";
done:
    close(commands[1]); close(reports[0]);
    if (strcmp(outcome, "passed") != 0) kill(pid, SIGKILL);
    int status = 0;
    pid_t waited;
    do { waited = waitpid(pid, &status, 0); } while (waited < 0 && errno == EINTR);
    if (waited != pid) { outcome = "failed"; stage = "child_reap"; error = errno; }
    else if (strcmp(outcome, "passed") == 0 && (!WIFEXITED(status) || WEXITSTATUS(status) != 0)) {
        outcome = "failed"; stage = "child_exit";
    }
    for (size_t i = 0; i < 5; i++) {
        int fd = (int[]){process_handle, thread_handle, wrong, pinned, replacement}[i];
        if (fd >= 0) close(fd);
    }
    printf("{\"outcome\":\"%s\",\"stage\":\"%s\",\"errno\":%d,"
        "\"leader_domain\":%d,\"thread_domain\":%d,\"replacement_domain\":%d,\"pinned_domain\":%d,"
        "\"copied_address_changed\":%s,\"guest_observed_bind\":%s,\"pinned_tcp_roundtrip\":%s,"
        "\"thread_after_exit_errno\":%d,\"nondumpable_fd_errno\":%d,\"nondumpable_memory_errno\":%d}\n",
        outcome, stage, error, leader_domain, thread_domain, replacement_domain, pinned_domain,
        copied_changed ? "true" : "false", guest_bound ? "true" : "false", tcp ? "true" : "false",
        thread_gone, private_fd, private_memory);
    return strcmp(outcome, "failed") == 0 ? 1 : 0;
}
