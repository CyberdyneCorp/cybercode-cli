/* Fault injection tests the actual send/result helpers without a pathname race.
 * This is verifier regression evidence, never native confinement acceptance. */
#include <stdio.h>
#include <errno.h>
#include <sys/socket.h>

static ssize_t injected_return;
static int injected_errno, send_calls;
static ssize_t controlled_sendmsg(int fd, const struct msghdr *message, int flags) {
    (void)fd; (void)message; (void)flags;
    send_calls++;
    errno = injected_errno;
    return injected_return;
}
#define sendmsg controlled_sendmsg
#include "macos_datagram_send.h"
#undef sendmsg

static int check(const char *name, ssize_t returned, int stale_error, char expected,
                 const char *stage, int error) {
    injected_return = returned; injected_errno = stale_error; send_calls = 0;
    struct unix_attempt sent = datagram_send(-1, "/controlled.sock", 2);
    struct datagram_failure failure = {0};
    char outcome = datagram_send_failure(sent, &failure);
    printf("{\"case\":\"%s\",\"attempted\":%d,\"calls\":%d,\"returned\":%zd,"
           "\"error\":%d,\"outcome\":\"%c\",\"stage\":\"%s\",\"has_return\":%d}\n",
           name, sent.attempted, send_calls, sent.returned, sent.error, outcome,
           failure.stage ? failure.stage : "", failure.has_return);
    int ok = send_calls == 1 && sent.attempted && !sent.sent && sent.returned == returned &&
        sent.error == error && outcome == expected;
    if (expected == 'E') {
        ok &= failure.stage && strcmp(failure.stage, stage) == 0 &&
            failure.error == error && failure.has_return && failure.returned == returned;
        /* A later error must not overwrite the first operation's raw witness. */
        struct unix_attempt later = { .attempted = 1, .returned = -1, .error = EBADF };
        (void)datagram_send_failure(later, &failure);
        ok &= failure.stage && strcmp(failure.stage, stage) == 0 &&
            failure.error == error && failure.has_return && failure.returned == returned;
    } else {
        ok &= !failure.stage && !failure.has_return;
    }
    if (!ok) fprintf(stderr, "send-result regression failed: %s\n", name);
    return ok;
}

int main(void) {
    if (!check("zero-stale-EPERM", 0, EPERM, 'E', "send_size", 0) ||
        !check("short-stale-EACCES", 7, EACCES, 'E', "send_size", 0) ||
        !check("failed-EACCES", -1, EACCES, 'D', "", EACCES) ||
        !check("failed-EPERM", -1, EPERM, 'D', "", EPERM) ||
        !check("failed-EINVAL", -1, EINVAL, 'E', "send", EINVAL)) return 1;
    char oversized[sizeof(((struct sockaddr_un *)0)->sun_path) + 1];
    memset(oversized, 'x', sizeof(oversized) - 1); oversized[sizeof(oversized) - 1] = 0;
    errno = EPERM; send_calls = 0;
    struct unix_attempt sent = datagram_send(-1, oversized, 2);
    struct datagram_failure failure = {0};
    char outcome = datagram_send_failure(sent, &failure);
    if (sent.attempted || send_calls || sent.error != ENAMETOOLONG || outcome != 'E' ||
        !failure.stage || strcmp(failure.stage, "send_setup") || failure.has_return) {
        fputs("send-result regression failed: setup-with-stale-EPERM\n", stderr); return 1;
    }
    if (datagram_socket(oversized) != -1 || errno != ENAMETOOLONG) {
        fputs("send-result regression failed: oversized-socket-address\n", stderr); return 1;
    }
    puts("{\"case\":\"setup-with-stale-EPERM\",\"attempted\":0,\"calls\":0,\"outcome\":\"E\",\"stage\":\"send_setup\",\"has_return\":0}");
    return 0;
}
