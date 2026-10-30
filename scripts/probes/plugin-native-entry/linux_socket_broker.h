/* Isolated experiment only. Included after confine() in a copied C launcher. */
#ifndef CYBER_SOCKET_BROKER_H
#define CYBER_SOCKET_BROKER_H
#include <netinet/in.h>
#include <signal.h>
#include <sys/time.h>
#include <sys/uio.h>
#include <sys/wait.h>

#ifndef PIDFD_THREAD
#define PIDFD_THREAD O_EXCL
#endif
#define BROKER_SLOTS 16

struct broker_result { int value; int error; };
struct broker_job { pid_t pid; int pipe; uint64_t id; };
struct broker_call {
    int socket;
    int endpoint;
    int operation;
    int backlog;
    socklen_t length;
    struct sockaddr_storage address;
};

#ifdef CYBER_PRIVATE_UNIX_STREAM
#include "socket_private_unix.h"
#endif

static void broker_tick(int signal_number) { (void)signal_number; }

static void broker_reply(int listener, uint64_t id, int value, int error) {
    struct seccomp_notif_resp response = { .id = id, .val = value, .error = -error };
    if (ioctl(listener, SECCOMP_IOCTL_NOTIF_ID_VALID, &id) == 0) {
        /* No CONTINUE: guest pointer/descriptor values are never re-resolved. */
        (void)ioctl(listener, SECCOMP_IOCTL_NOTIF_SEND, &response);
    }
}

/* Returns a pinned socket, or an errno. No worker reads guest memory. */
static int broker_copy(int listener, const struct seccomp_notif *request,
                       struct broker_call *call) {
    call->socket = -1;
    call->endpoint = -1;
    call->operation = request->data.nr;
    if (request->flags != 0 || request->pid == 0 || request->data.arch != CYBER_ARCH ||
        (call->operation != SYS_bind && call->operation != SYS_listen &&
         call->operation != SYS_connect)) return EPERM;
    if (ioctl(listener, SECCOMP_IOCTL_NOTIF_ID_VALID, &request->id) != 0) return ENOENT;
    int task = syscall(SYS_pidfd_open, request->pid, PIDFD_THREAD);
    if (task < 0) return errno;
    int socket = syscall(SYS_pidfd_getfd, task, (int)request->data.args[0], 0);
    int error = errno;
    close(task);
    if (socket < 0) return error;
    int domain = 0, type = 0;
    socklen_t size = sizeof(int);
    error = EPERM;
    if (getsockopt(socket, SOL_SOCKET, SO_DOMAIN, &domain, &size) != 0 ||
        size != sizeof(int)) goto failed;
    size = sizeof(int);
    if (getsockopt(socket, SOL_SOCKET, SO_TYPE, &type, &size) != 0 ||
        size != sizeof(int)) goto failed;
#ifdef CYBER_PRIVATE_UNIX_STREAM
    if (domain == AF_UNIX) {
        if (type != SOCK_STREAM || call->operation != SYS_connect) goto failed;
    } else
#endif
    if ((domain != AF_INET && domain != AF_INET6) ||
        (type != SOCK_STREAM && type != SOCK_DGRAM)) goto failed;
    if (call->operation == SYS_listen) {
        if (type != SOCK_STREAM) goto failed;
        call->backlog = (int)request->data.args[1];
    } else {
        uint64_t length = request->data.args[2];
        error = EINVAL;
        if (length < sizeof(sa_family_t) || length > sizeof(call->address)) goto failed;
        call->length = (socklen_t)length;
        struct iovec local = { .iov_base = &call->address, .iov_len = call->length };
        struct iovec remote = {
            .iov_base = (void *)(uintptr_t)request->data.args[1], .iov_len = call->length,
        };
        ssize_t copied = process_vm_readv(request->pid, &local, 1, &remote, 1, 0);
        if (copied != (ssize_t)call->length) {
            error = copied < 0 ? errno : EFAULT;
            goto failed;
        }
        int family = call->address.ss_family;
        error = EPERM;
#ifdef CYBER_PRIVATE_UNIX_STREAM
        if (domain == AF_UNIX) {
            error = private_endpoint(&call->address, call->length, request->pid, &call->endpoint);
            if (error != 0) goto failed;
        } else
#endif
        if (family == AF_UNSPEC && call->operation == SYS_connect) {
            /* Only IP objects reach here; UNIX pairs cannot disconnect/rebind. */
        } else if (family != domain ||
                   (domain == AF_INET && length < sizeof(struct sockaddr_in)) ||
                   (domain == AF_INET6 && length < sizeof(struct sockaddr_in6))) goto failed;
    }
    error = ENOENT;
    if (ioctl(listener, SECCOMP_IOCTL_NOTIF_ID_VALID, &request->id) != 0) goto failed;
    call->socket = socket;
    return 0;
failed:
    if (call->endpoint >= 0) { close(call->endpoint); call->endpoint = -1; }
    close(socket);
    return error;
}

static void broker_worker(struct broker_call call, int result_pipe, pid_t parent) {
    if (prctl(PR_SET_PDEATHSIG, SIGKILL, 0, 0, 0) != 0 || getppid() != parent) _exit(125);
    /* Duplicate above both fixed slots first, avoiding dup2 alias clobbering. */
    int socket = fcntl(call.socket, F_DUPFD_CLOEXEC, 6);
    int output = fcntl(result_pipe, F_DUPFD_CLOEXEC, 6);
    int endpoint = call.endpoint < 0 ? -1 : fcntl(call.endpoint, F_DUPFD_CLOEXEC, 6);
    if (socket < 0 || output < 0 || dup2(socket, 3) != 3 || dup2(output, 4) != 4 ||
        (call.endpoint >= 0 && (endpoint < 0 || dup2(endpoint, 5) != 5)) ||
        syscall(SYS_close_range, 6U, UINT_MAX, 0) != 0) _exit(125);
    if (call.endpoint < 0) close(5);
    close(0); close(1); close(2);
#ifdef CYBER_PRIVATE_UNIX_STREAM
    if (call.endpoint >= 0) {
        struct sockaddr_un *address = (struct sockaddr_un *)&call.address;
        memset(address, 0, sizeof(*address));
        address->sun_family = AF_UNIX;
        strcpy(address->sun_path, "/proc/self/fd/5");
        call.length = sizeof(*address);
    }
#endif
    int value;
    if (call.operation == SYS_bind) value = bind(3, (struct sockaddr *)&call.address, call.length);
    else if (call.operation == SYS_listen) value = listen(3, call.backlog);
    else value = connect(3, (struct sockaddr *)&call.address, call.length);
    struct broker_result result = { .value = value, .error = value < 0 ? errno : 0 };
    ssize_t written;
    do { written = write(4, &result, sizeof(result)); } while (written < 0 && errno == EINTR);
    _exit(written == (ssize_t)sizeof(result) ? 0 : 125);
}

static int broker_start(int listener, const struct seccomp_notif *request,
                        struct broker_job jobs[BROKER_SLOTS]) {
    int slot = 0;
    while (slot < BROKER_SLOTS && jobs[slot].pid != 0) slot++;
    if (slot == BROKER_SLOTS) return EAGAIN;
    struct broker_call call = {0};
    int error = broker_copy(listener, request, &call);
    if (error != 0) return error;
    int channel[2];
    if (pipe2(channel, O_CLOEXEC | O_NONBLOCK) != 0) {
        error = errno;
        close(call.socket);
        if (call.endpoint >= 0) close(call.endpoint);
        return error;
    }
    pid_t parent = getpid();
    pid_t worker = fork();
    error = errno;
    if (worker == 0) broker_worker(call, channel[1], parent);
    close(call.socket);
    if (call.endpoint >= 0) close(call.endpoint);
    close(channel[1]);
    if (worker < 0) { close(channel[0]); return error; }
    jobs[slot] = (struct broker_job){ .pid = worker, .pipe = channel[0], .id = request->id };
    return 0;
}

static void broker_settle_job(int listener, struct broker_job *job, short events) {
    if (job->pid == 0) return;
    if (job->pipe >= 0) {
        if (ioctl(listener, SECCOMP_IOCTL_NOTIF_ID_VALID, &job->id) != 0) {
            /* Identity is an owned, unreaped child, never a cached guest PID. */
            (void)kill(job->pid, SIGKILL);
            close(job->pipe);
            job->pipe = -1;
        } else if (events & (POLLIN | POLLHUP | POLLERR)) {
            struct broker_result result;
            ssize_t count = read(job->pipe, &result, sizeof(result));
            if (count == (ssize_t)sizeof(result) &&
                ((result.value == 0 && result.error == 0) ||
                 (result.value == -1 && result.error > 0 && result.error < 4096))) {
                broker_reply(listener, job->id, result.value, result.error);
            } else broker_reply(listener, job->id, -1, EIO);
            close(job->pipe);
            job->pipe = -1;
        }
    }
    int status;
    /* Drain the result before reaping, including a reply written just after
     * poll. An exited worker's pipe will become readable/HUP on the next tick. */
    if (job->pipe < 0 && waitpid(job->pid, &status, WNOHANG) == job->pid) {
        *job = (struct broker_job){0};
    }
}

static void broker_reap(pid_t child) {
    int status;
    while (waitpid(child, &status, 0) < 0 && errno == EINTR) {}
}

static int broker_loop(int listener, pid_t guest, int shell) {
    struct seccomp_notif_sizes sizes;
    if (syscall(SYS_seccomp, SECCOMP_GET_NOTIF_SIZES, 0, &sizes) != 0 ||
        sizes.seccomp_notif < sizeof(struct seccomp_notif)) {
        (void)kill(guest, SIGKILL); broker_reap(guest); return 2;
    }
    struct seccomp_notif *request = calloc(1, sizes.seccomp_notif);
    if (request == NULL) { (void)kill(guest, SIGKILL); broker_reap(guest); return 2; }
    struct broker_job jobs[BROKER_SLOTS] = {0};
    struct sigaction action = { .sa_handler = broker_tick };
    sigemptyset(&action.sa_mask);
    int guest_reaped = 0, status = 0, result = 2;
    int initial_exec = !shell;
    if (sigaction(SIGALRM, &action, NULL) != 0) goto done;
    for (;;) {
        pid_t observed = waitpid(guest, &status, WNOHANG);
        if (observed == guest) {
            guest_reaped = 1;
            result = WIFEXITED(status) ? WEXITSTATUS(status) : 128 + WTERMSIG(status);
            break;
        }
        if (observed < 0) {
            if (errno == EINTR) continue;
            if (errno == ECHILD) guest_reaped = 1;
            goto done;
        }
        struct pollfd pending[BROKER_SLOTS + 1] = {{ .fd = listener, .events = POLLIN }};
        for (int i = 0; i < BROKER_SLOTS; i++) pending[i + 1] =
            (struct pollfd){ .fd = jobs[i].pid == 0 ? -1 : jobs[i].pipe, .events = POLLIN };
        int ready = poll(pending, BROKER_SLOTS + 1, 100);
        if (ready < 0 && errno != EINTR) goto done;
        for (int i = 0; i < BROKER_SLOTS; i++) broker_settle_job(listener, &jobs[i], pending[i + 1].revents);
        if (pending[0].revents & (POLLERR | POLLNVAL)) goto done;
        if (pending[0].revents & POLLIN) {
            memset(request, 0, sizes.seccomp_notif);
            /* A signal may cancel a request between poll and RECV. Bound that
             * blocking receive without assuming O_NONBLOCK ioctl semantics. */
            alarm(1);
            int received = ioctl(listener, SECCOMP_IOCTL_NOTIF_RECV, request);
            int error = errno;
            alarm(0);
            if (received != 0) {
                if (error == EINTR || error == ENOENT) continue;
                goto done;
            }
            if (request->data.nr == SYS_execve) {
                if (initial_exec && request->pid == (unsigned int)guest &&
                    request->flags == 0 && request->data.arch == CYBER_ARCH &&
                    ioctl(listener, SECCOMP_IOCTL_NOTIF_ID_VALID, &request->id) == 0) {
                    /* The dedicated C guest has not executed any untrusted
                     * instruction or created a thread. Burn this budget before
                     * replying; no later request can acquire startup authority. */
                    initial_exec = 0;
                    struct seccomp_notif_resp response = {
                        .id = request->id, .flags = SECCOMP_USER_NOTIF_FLAG_CONTINUE,
                    };
                    if (ioctl(listener, SECCOMP_IOCTL_NOTIF_SEND, &response) != 0) goto done;
                } else broker_reply(listener, request->id, -1, ENOSYS);
                continue;
            }
            error = broker_start(listener, request, jobs);
            if (error != 0) broker_reply(listener, request->id, -1, error);
        }
    }
done:
    alarm(0);
    for (int i = 0; i < BROKER_SLOTS; i++) {
        if (jobs[i].pid == 0) continue;
        (void)kill(jobs[i].pid, SIGKILL);
        broker_reap(jobs[i].pid);
        if (jobs[i].pipe >= 0) close(jobs[i].pipe);
    }
    if (!guest_reaped) { (void)kill(guest, SIGKILL); broker_reap(guest); }
    free(request);
    return result;
}

static int broker_entry(const char *const argv[], int shell) {
#ifdef CYBER_PRIVATE_UNIX_STREAM
    if (private_initialize(argv[1]) != 0) return 2;
#endif
    int channel[2];
    struct sigaction child_action = { .sa_handler = SIG_DFL };
    sigemptyset(&child_action.sa_mask);
    sigset_t required_signals;
    sigemptyset(&required_signals);
    sigaddset(&required_signals, SIGALRM);
    sigaddset(&required_signals, SIGCHLD);
    if (sigprocmask(SIG_UNBLOCK, &required_signals, NULL) != 0 ||
        sigaction(SIGCHLD, &child_action, NULL) != 0 ||
        prctl(PR_SET_DUMPABLE, 0, 0, 0, 0) != 0 ||
        socketpair(AF_UNIX, SOCK_SEQPACKET | SOCK_CLOEXEC, 0, channel) != 0) return 2;
    pid_t parent = getpid();
    pid_t guest = fork();
    if (guest < 0) { close(channel[0]); close(channel[1]); return 2; }
    if (guest == 0) {
        close(channel[0]);
        if (prctl(PR_SET_PDEATHSIG, SIGKILL, 0, 0, 0) != 0 || getppid() != parent ||
            prctl(PR_SET_DUMPABLE, 1, 0, 0, 0) != 0 || confine(argv[1]) != 0) _exit(2);
#ifdef CYBER_PRIVATE_UNIX_STREAM
        if (private_make_sock() != 0) _exit(2);
        close(private_root);
#endif
        int listener = initial_filter(shell);
        int sent = listener < 0 ? -1 : transfer_listener(channel[1], listener);
        if (listener >= 0) close(listener);
        close(channel[1]);
        if (sent != 0) _exit(2);
        execv(argv[3], (char *const *)(argv + 3));
        _exit(2);
    }
    close(channel[1]); close(0); close(1);
    int listener = receive_listener(channel[0]);
    close(channel[0]);
    if (listener < 0) { (void)kill(guest, SIGKILL); broker_reap(guest); return 2; }
    int result = broker_loop(listener, guest, shell);
    close(listener);
#ifdef CYBER_PRIVATE_UNIX_STREAM
    close(private_root);
#endif
    return result;
}
#endif
