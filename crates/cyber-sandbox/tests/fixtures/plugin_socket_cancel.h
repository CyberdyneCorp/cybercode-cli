/* Controlled cancellation fixture; contacts only its private loopback server. */
#ifdef __linux__
#include <netinet/tcp.h>
#include <signal.h>
#include <time.h>

struct cancel_call { int fd; struct sockaddr_in address; int result; int error; };
static void cancel_signal(int number) { (void)number; }
static void *cancel_connect(void *argument) {
    struct cancel_call *call = argument;
    call->result = connect(call->fd, (struct sockaddr *)&call->address, sizeof(call->address));
    call->error = call->result < 0 ? errno : 0;
    return NULL;
}
static int syn_pending(int fd) {
    struct timespec start, now;
    if (clock_gettime(CLOCK_MONOTONIC, &start) != 0) return 0;
    do {
        struct tcp_info info = {0};
        socklen_t size = sizeof(info);
        if (getsockopt(fd, IPPROTO_TCP, TCP_INFO, &info, &size) != 0) return 0;
        if (info.tcpi_state == TCP_SYN_SENT) return 1;
        struct timespec pause = { .tv_nsec = 1000000 };
        nanosleep(&pause, NULL);
        if (clock_gettime(CLOCK_MONOTONIC, &now) != 0) return 0;
    } while (now.tv_sec - start.tv_sec < 10);
    return 0;
}
static int cancel_probe(void) {
    struct sigaction action = { .sa_handler = cancel_signal };
    sigemptyset(&action.sa_mask);
    if (sigaction(SIGUSR1, &action, NULL) != 0) return 3;
    int server = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0);
    int filler = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0);
    int pending = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (server < 0 || filler < 0 || pending < 0) return 3;
    int retries = 10;
    if (setsockopt(pending, IPPROTO_TCP, TCP_SYNCNT, &retries, sizeof(retries)) != 0) return 3;
    struct sockaddr_in address = { .sin_family = AF_INET,
        .sin_addr.s_addr = htonl(INADDR_LOOPBACK) };
    socklen_t length = sizeof(address);
    /* backlog zero admits one completed connection on Linux. Keep it queued
     * without accept, making the next blocking connect enter SYN_SENT. */
    if (bind(server, (struct sockaddr *)&address, length) != 0 || listen(server, 0) != 0 ||
        getsockname(server, (struct sockaddr *)&address, &length) != 0 ||
        connect(filler, (struct sockaddr *)&address, length) != 0) return 3;
    struct cancel_call call = { .fd = pending, .address = address };
    pthread_t thread;
    if (pthread_create(&thread, NULL, cancel_connect, &call) != 0) return 3;
    int started = syn_pending(pending);
    if (pthread_kill(thread, SIGUSR1) != 0 || pthread_join(thread, NULL) != 0) _exit(3);
    int cancelled = call.result == -1 && call.error == EINTR;
    printf("{\"phase\":\"cancel-ready\",\"client_syn_sent\":%s,\"cancelled\":%s}\n",
        started ? "true" : "false", cancelled ? "true" : "false");
    fflush(stdout);
    /* The trusted test driver releases this only after observing the exact
     * cancelled worker's normal waitpid receipt. No fixed cleanup sleep. */
    if (getchar() != 'R') return 3;
    close(pending); close(filler); close(server);
    int alive = local_exchange(AF_INET, NULL, -1);
    printf("{\"phase\":\"cancel-finished\",\"tcp_after_cancel\":%s}\n", alive ? "true" : "false");
    return started && cancelled && alive ? 0 : 3;
}
#else
static int cancel_probe(void) { return 3; }
#endif
