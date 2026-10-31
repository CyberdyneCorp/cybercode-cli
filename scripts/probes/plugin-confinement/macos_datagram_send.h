/* Shared send/result implementation; deterministic controls include these exact helpers. */
#ifndef CYBER_MACOS_DATAGRAM_SEND_H
#define CYBER_MACOS_DATAGRAM_SEND_H
#include <errno.h>
#include <string.h>
#include <sys/types.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <sys/uio.h>
#include <sys/un.h>
#include <unistd.h>

struct unix_attempt {
    int connected, sent, acknowledged;
    /* Datagram diagnostics retain errno only for a real failed send. */
    int attempted, error;
    ssize_t returned;
};
struct datagram_failure { const char *stage; int error, has_return; ssize_t returned; };

static char datagram_failed(struct datagram_failure *failure, const char *stage, int error) {
    if (!failure->stage) { failure->stage = stage; failure->error = error; }
    return 'E';
}

static char datagram_send_failure(struct unix_attempt sent, struct datagram_failure *failure) {
    if (sent.attempted && sent.returned == -1 && (sent.error == EACCES || sent.error == EPERM)) return 'D';
    if (!failure->stage && sent.attempted) {
        failure->returned = sent.returned; failure->has_return = 1;
    }
    const char *stage = !sent.attempted ? "send_setup" : sent.returned == -1 ? "send" : "send_size";
    return datagram_failed(failure, stage, sent.error);
}

static int datagram_address(const char *path, struct sockaddr_un *address) {
    if (strlen(path) >= sizeof(address->sun_path)) return 0;
    memset(address, 0, sizeof(*address));
    address->sun_family = AF_UNIX;
    strcpy(address->sun_path, path);
    return 1;
}

static int datagram_socket(const char *path) {
    struct sockaddr_un address;
    if (!datagram_address(path, &address)) { errno = ENAMETOOLONG; return -1; }
    int fd = socket(AF_UNIX, SOCK_DGRAM, 0);
    if (fd < 0) return -1;
    struct timeval timeout = { .tv_sec = 2 };
    if (setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout)) != 0 ||
        setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &timeout, sizeof(timeout)) != 0 ||
        bind(fd, (struct sockaddr *)&address, sizeof(address)) != 0) {
        int error = errno;
        close(fd); errno = error; return -1;
    }
    return fd;
}

static struct unix_attempt datagram_send(int fd, const char *path, int method) {
    struct unix_attempt result = { .returned = -1 };
    struct sockaddr_un address;
    if (!datagram_address(path, &address)) { result.error = ENAMETOOLONG; return result; }
    if (method == 0) {
        result.connected = connect(fd, (struct sockaddr *)&address, sizeof(address)) == 0;
        if (!result.connected) { result.error = errno; return result; }
    }
    result.attempted = 1;
    if (method == 0) {
        result.returned = send(fd, "fixture-only", 12, 0);
    } else if (method == 1) {
        result.returned = sendto(fd, "fixture-only", 12, 0, (struct sockaddr *)&address, sizeof(address));
    } else {
        struct iovec body = { .iov_base = (void *)"fixture-only", .iov_len = 12 };
        struct msghdr message = { .msg_name = &address, .msg_namelen = sizeof(address), .msg_iov = &body, .msg_iovlen = 1 };
        result.returned = sendmsg(fd, &message, 0);
    }
    result.error = result.returned == -1 ? errno : 0;
    result.sent = result.returned == 12;
    return result;
}

#endif
