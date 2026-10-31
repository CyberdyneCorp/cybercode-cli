/* Owned-fixture endpoint identity prerequisite; not a confinement boundary. */
#define _GNU_SOURCE
#ifdef __linux__
#include <errno.h>
#include <fcntl.h>
#include <linux/openat2.h>
#include <poll.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/un.h>
#include <unistd.h>

static int endpoint(int directory, const char *name) {
    struct open_how how = { .flags = O_PATH | O_CLOEXEC,
        .resolve = RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS | RESOLVE_NO_SYMLINKS };
    int fd = (int)syscall(SYS_openat2, directory, name, &how, sizeof(how));
    if (fd < 0) return -1;
    struct stat info;
    if (fstat(fd, &info) != 0 || !S_ISSOCK(info.st_mode)) {
        close(fd);
        errno = ENOTSOCK;
        return -1;
    }
    return fd;
}
static int address(struct sockaddr_un *value, const char *path) {
    memset(value, 0, sizeof(*value));
    value->sun_family = AF_UNIX;
    int size = snprintf(value->sun_path, sizeof(value->sun_path), "%s", path);
    return size > 0 && (size_t)size < sizeof(value->sun_path) ? 0 : -1;
}
static int bound(int type, const char *path) {
    struct sockaddr_un value;
    if (address(&value, path) != 0) return -1;
    int fd = socket(AF_UNIX, type | SOCK_CLOEXEC | SOCK_NONBLOCK, 0);
    if (fd < 0) return -1;
    if (bind(fd, (struct sockaddr *)&value, sizeof(value)) != 0 ||
        (type != SOCK_DGRAM && listen(fd, 1) != 0)) {
        int error = errno;
        close(fd);
        errno = error;
        return -1;
    }
    return fd;
}
static int ready(int fd) {
    struct pollfd value = { .fd = fd, .events = POLLIN };
    int result;
    do { result = poll(&value, 1, 1000); } while (result < 0 && errno == EINTR);
    return result == 1 && (value.revents & POLLIN) != 0;
}
static int exchange(int client, int listener, int type) {
    int peer = listener;
    if (type != SOCK_DGRAM) {
        peer = accept4(listener, NULL, NULL, SOCK_CLOEXEC | SOCK_NONBLOCK);
        if (peer < 0) return 0;
    }
    const char token = 'T', ack = 'A';
    char received = 0;
    struct sockaddr_un sender;
    socklen_t length = sizeof(sender);
    int passed = send(client, &token, 1, MSG_NOSIGNAL) == 1 && ready(peer) &&
        recvfrom(peer, &received, 1, 0, (struct sockaddr *)&sender, &length) == 1 &&
        received == token;
    if (passed) {
        ssize_t sent = type == SOCK_DGRAM
            ? sendto(peer, &ack, 1, MSG_NOSIGNAL, (struct sockaddr *)&sender, length)
            : send(peer, &ack, 1, MSG_NOSIGNAL);
        passed = sent == 1 && ready(client) && recv(client, &received, 1, 0) == 1 &&
            received == ack;
    }
    if (peer != listener) close(peer);
    return passed;
}
static int refused(int directory, const char *path, int expected) {
    int fd = endpoint(directory, path);
    int error = errno;
    if (fd >= 0) close(fd);
    return fd < 0 && error == expected;
}
static int observe(int type, const char *label, int pinned) {
    char root[] = "/tmp/cyber-unix-pin-XXXXXX";
    if (mkdtemp(root) == NULL) return 0;
    char scratch[108], selected[108], ambient[108], client_path[108];
    snprintf(scratch, sizeof(scratch), "%s/scratch", root);
    snprintf(selected, sizeof(selected), "%s/scratch/endpoint", root);
    snprintf(ambient, sizeof(ambient), "%s/ambient", root);
    snprintf(client_path, sizeof(client_path), "%s/scratch/client", root);
    int directory = -1, private = -1, outside = -1, client = -1, pin = -1;
    int replaced = 0, linked_refused = 0, escape_refused = 0, absolute_refused = 0;
    int kind_refused = 0, identity_retained = 0, private_received = 0, ambient_received = 0;
    int roundtrip = 0, error = 0;
    const char *stage = "setup";
    if (mkdir(scratch, 0700) != 0) goto done;
    directory = open(scratch, O_PATH | O_DIRECTORY | O_CLOEXEC);
    private = bound(type, selected);
    outside = bound(type, ambient);
    if (directory < 0 || private < 0 || outside < 0) goto done;
    pin = endpoint(directory, "endpoint");
    struct stat before, after;
    if (pin < 0 || fstat(pin, &before) != 0) goto done;
    kind_refused = refused(directory, ".", ENOTSOCK);
    escape_refused = refused(directory, "../ambient", EXDEV);
    absolute_refused = refused(directory, ambient, EXDEV);
    stage = "replace";
    if (unlink(selected) != 0 || symlink("../ambient", selected) != 0) goto done;
    replaced = 1;
    linked_refused = refused(directory, "endpoint", ELOOP);
    identity_retained = fstat(pin, &after) == 0 && before.st_dev == after.st_dev &&
        before.st_ino == after.st_ino && after.st_nlink == 0;
    stage = "connect";
    client = socket(AF_UNIX, type | SOCK_CLOEXEC | SOCK_NONBLOCK, 0);
    struct sockaddr_un local, target;
    if (client < 0 || address(&local, client_path) != 0 ||
        bind(client, (struct sockaddr *)&local, sizeof(local)) != 0) goto done;
    char reference[64];
    snprintf(reference, sizeof(reference), "/proc/self/fd/%d", pin);
    if (address(&target, pinned ? reference : selected) != 0 ||
        connect(client, (struct sockaddr *)&target, sizeof(target)) != 0) goto done;
    /* The missing-pin control deliberately resolves the replaced pathname. All
       endpoints are this fixture's own dummy sockets, never ambient services. */
    int chosen = pinned ? private : outside;
    roundtrip = exchange(client, chosen, type);
    private_received = pinned && roundtrip;
    ambient_received = !pinned && roundtrip;
    struct pollfd other = { .fd = pinned ? outside : private, .events = POLLIN };
    if (poll(&other, 1, 0) != 0) roundtrip = 0;
    stage = "complete";
done:
    error = strcmp(stage, "complete") == 0 ? 0 : errno;
    if (pin >= 0) close(pin);
    if (client >= 0) close(client);
    if (outside >= 0) close(outside);
    if (private >= 0) close(private);
    if (directory >= 0) close(directory);
    unlink(client_path); unlink(selected); unlink(ambient);
    rmdir(scratch); rmdir(root);
    int passed = error == 0 && replaced && linked_refused && escape_refused &&
        absolute_refused && kind_refused && identity_retained && roundtrip &&
        private_received == pinned && ambient_received == !pinned;
    printf("{\"type\":\"%s\",\"pinned\":%s,\"stage\":\"%s\",\"errno\":%d,"
        "\"replaced\":%s,\"symlink_refused\":%s,\"escape_refused\":%s,"
        "\"absolute_refused\":%s,\"kind_refused\":%s,\"identity_retained\":%s,"
        "\"private_received\":%s,\"ambient_received\":%s,\"roundtrip\":%s,\"passed\":%s}\n",
        label, pinned ? "true" : "false", stage, error,
        replaced ? "true" : "false", linked_refused ? "true" : "false",
        escape_refused ? "true" : "false", absolute_refused ? "true" : "false",
        kind_refused ? "true" : "false", identity_retained ? "true" : "false",
        private_received ? "true" : "false", ambient_received ? "true" : "false",
        roundtrip ? "true" : "false", passed ? "true" : "false");
    return passed;
}
int main(void) {
    const int types[] = {SOCK_STREAM, SOCK_DGRAM, SOCK_SEQPACKET};
    const char *labels[] = {"stream", "datagram", "seqpacket"};
    int passed = 1;
    for (size_t i = 0; i < sizeof(types) / sizeof(types[0]); ++i)
        for (int pinned = 0; pinned <= 1; ++pinned)
            passed &= observe(types[i], labels[i], pinned);
    return passed ? 0 : 1;
}
#else
int main(void) { return 77; }
#endif
