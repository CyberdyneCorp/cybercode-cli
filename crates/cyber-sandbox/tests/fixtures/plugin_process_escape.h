/* A controlled escaped descendant remains releasable by closing owner stdin. */
#ifdef __linux__
#include <sys/stat.h>
static int escape_probe(void) {
    int ready[2];
    if (pipe2(ready, O_CLOEXEC) != 0) return 3;
    pid_t child = fork();
    if (child < 0) return 3;
    if (child == 0) {
        close(ready[0]);
        int escaped = setsid() == getpid();
        struct stat input;
        int flags = fcntl(STDIN_FILENO, F_GETFL);
        int controlled = flags >= 0 && (flags & O_ACCMODE) == O_RDONLY && !(flags & O_NONBLOCK) &&
            fstat(STDIN_FILENO, &input) == 0 && S_ISFIFO(input.st_mode);
        printf("{\"phase\":\"escape-ready\",\"escaped_session\":%s,\"controlled_stdin\":%s}\n",
            escaped ? "true" : "false", controlled ? "true" : "false");
        fflush(stdout);
        char receipt = escaped && controlled ? 'S' : 'F';
        if (write(ready[1], &receipt, 1) != 1) _exit(3);
        close(ready[1]);
        /* No PID lookup or signal is needed to clean the missing-namespace
         * control. The exact pipe owner closes this after measuring settlement. */
        char command;
        ssize_t count;
        do { count = read(STDIN_FILENO, &command, 1); } while (count > 0 || (count < 0 && errno == EINTR));
        _exit(count == 0 && escaped && controlled ? 0 : 3);
    }
    close(ready[1]);
    char receipt = 0;
    ssize_t count;
    do { count = read(ready[0], &receipt, 1); } while (count < 0 && errno == EINTR);
    close(ready[0]);
    /* Exit only after the descendant really owns another session and has
     * flushed its stdout receipt. It intentionally retains stdout and stdin. */
    return count == 1 && receipt == 'S' ? 0 : 3;
}
#else
static int escape_probe(void) { return 3; }
#endif
