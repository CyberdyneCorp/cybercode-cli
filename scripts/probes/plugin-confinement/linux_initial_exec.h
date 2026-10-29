/* Experimental initial-exec admission for the trusted single-threaded launcher.
 * No untrusted code may run until the single exec notification is consumed.
 * The supervisor does not inspect mutable pointers or authorize later requests.
 * Policy replacement must be denied: a newer USER_NOTIF filter could otherwise
 * replace the listener and allow a subsequent exec. Not production packaging. */
#ifndef CYBER_PROOF_LINUX_INITIAL_EXEC_H
#define CYBER_PROOF_LINUX_INITIAL_EXEC_H
#include "linux_filter.h"
#include <poll.h>
#include <stdlib.h>
#include <sys/ioctl.h>
#include <sys/socket.h>

static int initial_filter(int shell) {
    unsigned int execute = shell ? SECCOMP_RET_ALLOW : SECCOMP_RET_USER_NOTIF;
    unsigned int spawn = shell ? SECCOMP_RET_ALLOW : SECCOMP_RET_ERRNO | EPERM;
    unsigned int thread_mask = shell ? 0 : CLONE_THREAD | CLONE_VM | CLONE_SIGHAND;
    struct sock_filter instructions[] = {
        PROOF_ARCH_CHECK,
        PROOF_ESCAPE_DENIALS,
        PROOF_DENY(SYS_seccomp),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_prctl, 0, 3),
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, args[0])),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, PR_SET_SECCOMP, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM),
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, nr)),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_execve, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, execute),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_execveat, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, spawn),
#ifdef SYS_fork
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_fork, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, spawn),
#endif
#ifdef SYS_vfork
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_vfork, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, spawn),
#endif
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_clone3, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, shell ? SECCOMP_RET_ALLOW : SECCOMP_RET_ERRNO | ENOSYS),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_clone, 0, 4),
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, args[0])),
        BPF_STMT(BPF_ALU | BPF_AND | BPF_K, thread_mask),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, thread_mask, 1, 0),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    struct sock_fprog program = { .len = sizeof(instructions) / sizeof(instructions[0]), .filter = instructions };
    return syscall(SYS_seccomp, SECCOMP_SET_MODE_FILTER,
                   shell ? 0 : SECCOMP_FILTER_FLAG_NEW_LISTENER, &program);
}

static int transfer_listener(int socket, int listener) {
    char marker = 'L';
    struct iovec body = { .iov_base = &marker, .iov_len = 1 };
    union { struct cmsghdr alignment; char bytes[CMSG_SPACE(sizeof(int))]; } control = {0};
    struct msghdr message = { .msg_iov = &body, .msg_iovlen = 1,
        .msg_control = control.bytes, .msg_controllen = sizeof(control.bytes) };
    struct cmsghdr *header = CMSG_FIRSTHDR(&message);
    header->cmsg_level = SOL_SOCKET;
    header->cmsg_type = SCM_RIGHTS;
    header->cmsg_len = CMSG_LEN(sizeof(int));
    memcpy(CMSG_DATA(header), &listener, sizeof(listener));
    return sendmsg(socket, &message, MSG_NOSIGNAL) == 1 ? 0 : -1;
}

static int wait_readable(int fd) {
    struct pollfd pending = { .fd = fd, .events = POLLIN };
    return poll(&pending, 1, 10000) == 1 && (pending.revents & POLLIN) != 0 ? 0 : -1;
}

static int receive_listener(int socket) {
    if (wait_readable(socket) != 0) return -1;
    char marker = 0;
    struct iovec body = { .iov_base = &marker, .iov_len = 1 };
    union { struct cmsghdr alignment; char bytes[CMSG_SPACE(sizeof(int))]; } control = {0};
    struct msghdr message = { .msg_iov = &body, .msg_iovlen = 1,
        .msg_control = control.bytes, .msg_controllen = sizeof(control.bytes) };
    if (recvmsg(socket, &message, MSG_CMSG_CLOEXEC) != 1 || marker != 'L' ||
        (message.msg_flags & (MSG_CTRUNC | MSG_TRUNC)) != 0) return -1;
    struct cmsghdr *header = CMSG_FIRSTHDR(&message);
    if (header == NULL || header->cmsg_level != SOL_SOCKET || header->cmsg_type != SCM_RIGHTS ||
        header->cmsg_len != CMSG_LEN(sizeof(int))) return -1;
    int fd;
    memcpy(&fd, CMSG_DATA(header), sizeof(fd));
    return fd;
}

static int continue_initial_exec(int listener, pid_t expected) {
    struct seccomp_notif_sizes sizes;
    if (syscall(SYS_seccomp, SECCOMP_GET_NOTIF_SIZES, 0, &sizes) != 0 ||
        sizes.seccomp_notif < sizeof(struct seccomp_notif) ||
        sizes.seccomp_notif_resp < sizeof(struct seccomp_notif_resp)) return -1;
    struct seccomp_notif *request = calloc(1, sizes.seccomp_notif);
    struct seccomp_notif_resp *response = calloc(1, sizes.seccomp_notif_resp);
    int result = -1;
    if (request == NULL || response == NULL || wait_readable(listener) != 0) goto done;
    if (ioctl(listener, SECCOMP_IOCTL_NOTIF_RECV, request) != 0 ||
        request->pid != (unsigned int)expected || request->flags != 0 ||
        request->data.arch != PROOF_ARCH || request->data.nr != SYS_execve ||
        ioctl(listener, SECCOMP_IOCTL_NOTIF_ID_VALID, &request->id) != 0) goto done;
    response->id = request->id;
    response->flags = SECCOMP_USER_NOTIF_FLAG_CONTINUE;
    result = ioctl(listener, SECCOMP_IOCTL_NOTIF_SEND, response);
done:
    free(request);
    free(response);
    return result;
}

static int initial_exec_boundary(int shell) {
    if (shell) return initial_filter(1) == 0 ? 0 : -1;
    int channel[2];
    if (socketpair(AF_UNIX, SOCK_SEQPACKET | SOCK_CLOEXEC, 0, channel) != 0) return -1;
    pid_t target = getpid();
    pid_t supervisor = fork();
    if (supervisor < 0) { close(channel[0]); close(channel[1]); return -1; }
    if (supervisor == 0) {
        // Even with a runtime's private-proc read roots, untrusted target code
        // must not inspect the supervisor's memory, environment or listener.
        if (prctl(PR_SET_DUMPABLE, 0, 0, 0, 0) != 0) _exit(2);
        close(channel[0]);
        close(STDIN_FILENO);
        close(STDOUT_FILENO);
        int listener = receive_listener(channel[1]);
        close(channel[1]);
        int result = listener < 0 ? -1 : continue_initial_exec(listener, target);
        if (listener >= 0) close(listener);
        /* Last listener close makes all future exec requests fail with ENOSYS.
         * This supervisor never authorizes a second request, even if queued. */
        _exit(result == 0 ? 0 : 2);
    }
    close(channel[1]);
    int listener = initial_filter(0);
    int result = listener < 0 ? -1 : transfer_listener(channel[0], listener);
    if (listener >= 0) close(listener);
    close(channel[0]);
    return result;
}
#endif
