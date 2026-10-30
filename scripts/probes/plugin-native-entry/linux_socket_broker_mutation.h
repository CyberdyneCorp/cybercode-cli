/* Included only in temporary mutation-fixture helpers. No production hook. */
static volatile sig_atomic_t broker_mutation_released;
static uint64_t broker_mutation_id;
static int broker_mutation_seen;
static void broker_mutation_signal(int number) {
    (void)number;
    broker_mutation_released = 1;
}
static int broker_mutation_pause(const struct seccomp_notif *request) {
    if (request->data.nr != SYS_bind || request->data.args[2] != sizeof(struct sockaddr_storage)) return 0;
    sigset_t blocked, previous, waiting;
    sigemptyset(&blocked);
    sigaddset(&blocked, SIGUSR2);
    if (sigprocmask(SIG_BLOCK, &blocked, &previous) != 0) return -1;
    struct sigaction action = { .sa_handler = broker_mutation_signal }, old;
    sigemptyset(&action.sa_mask);
    if (sigaction(SIGUSR2, &action, &old) != 0) {
        (void)sigprocmask(SIG_SETMASK, &previous, NULL);
        return -1;
    }
    broker_mutation_released = 0;
    broker_mutation_id = request->id;
    broker_mutation_seen = 1;
    waiting = previous;
    sigdelset(&waiting, SIGUSR2);
    int result = 1;
    if (fprintf(stderr, "BPIN %u\n", request->pid) < 0 || fflush(stderr) != 0) result = -1;
    else while (!broker_mutation_released) sigsuspend(&waiting);
    int restored_action = sigaction(SIGUSR2, &old, NULL);
    int restored_mask = sigprocmask(SIG_SETMASK, &previous, NULL);
    return restored_action == 0 && restored_mask == 0 ? result : -1;
}
