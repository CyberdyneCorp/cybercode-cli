/* A native detached descendant whose lifetime is controlled by the test owner. */
#include <errno.h>
#include <stdio.h>
#include <unistd.h>

int main(void) {
    int ready[2];
    if (pipe(ready) != 0) return 2;
    pid_t child = fork();
    if (child < 0) return 3;
    if (child == 0) {
        close(ready[0]);
        /* Exercise the schedule that used to let the parent exit before setsid.
         * Correctness uses the pipe receipt, never this delay. */
        usleep(100000);
        if (setsid() < 0) _exit(4);
        const char receipt = 'R';
        if (write(ready[1], &receipt, 1) != 1) _exit(5);
        close(ready[1]);
        /* Keep stdio open until the owner has checked unknown settlement. The
         * ceiling only bounds cleanup after a failed/panicking test. */
        for (unsigned i = 0; i < 6000; ++i) {
            if (access("release-detached-fixture", F_OK) == 0) {
                FILE *finished = fopen("detached-fixture-finished", "w");
                if (finished != NULL) fclose(finished);
                _exit(0);
            }
            usleep(10000);
        }
        _exit(6);
    }
    close(ready[1]);
    char receipt = 0;
    ssize_t count;
    do { count = read(ready[0], &receipt, 1); } while (count < 0 && errno == EINTR);
    close(ready[0]);
    return count == 1 && receipt == 'R' ? 0 : 7;
}
