/* Independent fixture expectation; the wrong-cap control changes only the helper. */
#ifdef __linux__
#define FIXTURE_WORKER_LIMIT 16
static int saturation_probe(void) {
    struct sigaction action = { .sa_handler = cancel_signal };
    sigemptyset(&action.sa_mask);
    if (sigaction(SIGUSR1, &action, NULL) != 0) return 3;
    int server = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0);
    int filler = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (server < 0 || filler < 0) return 3;
    struct sockaddr_in address = { .sin_family = AF_INET,
        .sin_addr.s_addr = htonl(INADDR_LOOPBACK) };
    socklen_t length = sizeof(address);
    if (bind(server, (struct sockaddr *)&address, length) != 0 || listen(server, 0) != 0 ||
        getsockname(server, (struct sockaddr *)&address, &length) != 0 ||
        connect(filler, (struct sockaddr *)&address, length) != 0) return 3;
    puts("{\"phase\":\"saturation-ready\",\"setup_complete\":true}");
    fflush(stdout);
    /* Wait until the owner has reaped all three setup workers. Otherwise a
     * just-completed setup request could transiently consume one of the slots. */
    if (getchar() != 'R') return 3;
    struct cancel_call calls[FIXTURE_WORKER_LIMIT] = {0};
    pthread_t threads[FIXTURE_WORKER_LIMIT];
    for (int i = 0; i < FIXTURE_WORKER_LIMIT; i++) {
        calls[i].fd = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0);
        int retries = 10;
        if (calls[i].fd < 0 || setsockopt(calls[i].fd, IPPROTO_TCP, TCP_SYNCNT, &retries, sizeof(retries)) != 0) _exit(3);
        calls[i].address = address;
        if (pthread_create(&threads[i], NULL, cancel_connect, &calls[i]) != 0) _exit(3);
    }
    int blocked = 0;
    for (int i = 0; i < FIXTURE_WORKER_LIMIT; i++) blocked += syn_pending(calls[i].fd);
    printf("{\"phase\":\"saturation-filled\",\"blocked_count\":%d}\n", blocked);
    fflush(stdout);
    int extra = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (extra < 0) _exit(3);
    int value = connect(extra, (struct sockaddr *)&address, length);
    int backpressure = value == -1 && errno == EAGAIN;
    close(extra);
    for (int i = 0; i < FIXTURE_WORKER_LIMIT; i++) {
        if (pthread_kill(threads[i], SIGUSR1) != 0) _exit(3);
    }
    int interrupted = 0;
    for (int i = 0; i < FIXTURE_WORKER_LIMIT; i++) {
        if (pthread_join(threads[i], NULL) != 0) _exit(3);
        interrupted += calls[i].result == -1 && calls[i].error == EINTR;
    }
    printf("{\"phase\":\"saturation-cancelled\",\"backpressure\":%s,\"interrupted_count\":%d}\n",
        backpressure ? "true" : "false", interrupted);
    fflush(stdout);
    /* Hold the socket preimages until every cancelled worker is acknowledged
     * reaped. A fresh TCP exchange then verifies capacity is available again. */
    if (getchar() != 'R') _exit(3);
    for (int i = 0; i < FIXTURE_WORKER_LIMIT; i++) close(calls[i].fd);
    close(filler); close(server);
    int alive = local_exchange(AF_INET, NULL, -1);
    printf("{\"phase\":\"saturation-finished\",\"tcp_after_saturation\":%s}\n", alive ? "true" : "false");
    return blocked == FIXTURE_WORKER_LIMIT && backpressure && interrupted == FIXTURE_WORKER_LIMIT && alive ? 0 : 3;
}
#else
static int saturation_probe(void) { return 3; }
#endif
