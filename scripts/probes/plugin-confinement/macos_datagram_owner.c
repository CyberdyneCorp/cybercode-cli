/* Trusted diagnostic owner for one fixture socket and one exact client path. */
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stddef.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <time.h>
#include <unistd.h>

static int drain(int server, const char *client, int *receipts) {
    for (;;) {
        struct sockaddr_un peer = {0};
        socklen_t length = sizeof(peer);
        char bytes[13];
        ssize_t size = recvfrom(server, bytes, sizeof(bytes), 0, (struct sockaddr *)&peer, &length);
        if (size < 0 && errno == EINTR) continue;
        if (size < 0) return errno == EAGAIN || errno == EWOULDBLOCK ? 0 : -1;
        if (size != 12 || memcmp(bytes, "fixture-only", 12) != 0 ||
            length <= offsetof(struct sockaddr_un, sun_path) || length > sizeof(peer) ||
            peer.sun_family != AF_UNIX ||
            memchr(peer.sun_path, 0, length - offsetof(struct sockaddr_un, sun_path)) == NULL ||
            strcmp(peer.sun_path, client) != 0 || ++*receipts > 32) return -1;
        if (sendto(server, "A", 1, 0, (struct sockaddr *)&peer, length) != 1) return -1;
    }
}

int main(int argc, char **argv) {
    struct sockaddr_un address = { .sun_family = AF_UNIX };
    if (argc != 3 || strlen(argv[1]) >= sizeof(address.sun_path) ||
        strlen(argv[2]) >= sizeof(address.sun_path)) return 2;
    strcpy(address.sun_path, argv[1]);
    int server = socket(AF_UNIX, SOCK_DGRAM, 0);
    if (server < 0) return 2;
    int flags = fcntl(server, F_GETFL);
    if (flags < 0 || fcntl(server, F_SETFL, flags | O_NONBLOCK) != 0 ||
        bind(server, (struct sockaddr *)&address, sizeof(address)) != 0) { close(server); return 2; }
    struct timespec started, now;
    if (clock_gettime(CLOCK_MONOTONIC, &started) != 0) { close(server); unlink(argv[1]); return 2; }
    puts("READY");
    fflush(stdout);
    int receipts = 0, result = 2;
    for (;;) {
        if (clock_gettime(CLOCK_MONOTONIC, &now) != 0 || now.tv_sec - started.tv_sec >= 30) break;
        struct pollfd descriptors[2] = { { .fd = server, .events = POLLIN }, { .fd = 0, .events = POLLIN } };
        int ready = poll(descriptors, 2, 100);
        if (ready < 0 && errno == EINTR) continue;
        if (ready < 0 || descriptors[0].revents & (POLLERR | POLLHUP | POLLNVAL)) break;
        if (descriptors[0].revents & POLLIN && drain(server, argv[2], &receipts) != 0) break;
        if (descriptors[1].revents & (POLLIN | POLLHUP | POLLERR | POLLNVAL)) {
            char stop = 0;
            if (read(0, &stop, 1) != 1 || stop != 'S' || drain(server, argv[2], &receipts) != 0) break;
            printf("{\"stopped\":true,\"receipts\":%d}\n", receipts);
            result = 0;
            break;
        }
    }
    close(server);
    unlink(argv[1]);
    return result;
}
