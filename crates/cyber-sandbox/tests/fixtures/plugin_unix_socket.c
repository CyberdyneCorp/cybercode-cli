/* Fixture-only connection probe. It never contacts a non-fixture service. */
#include <errno.h>
#include <fcntl.h>
#include <stddef.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <sys/uio.h>
#include <unistd.h>
#include <netinet/in.h>
#ifdef __linux__
#include <linux/landlock.h>
#include <sys/syscall.h>
#endif

static int abi(void) {
#ifdef __linux__
    return (int)syscall(SYS_landlock_create_ruleset, NULL, 0, LANDLOCK_CREATE_RULESET_VERSION);
#else
    return 0;
#endif
}

static int local_exchange(int family, const char *path) {
    int server = -1, client = -1, peer = -1, ok = 0;
    struct sockaddr_storage storage = {0};
    socklen_t length;
    if (family == AF_UNIX) {
        struct sockaddr_un *address = (struct sockaddr_un *)&storage;
        if (strlen(path) >= sizeof(address->sun_path)) return 0;
        address->sun_family = AF_UNIX;
        memcpy(address->sun_path, path, strlen(path) + 1);
        length = (socklen_t)(offsetof(struct sockaddr_un, sun_path) + strlen(path) + 1);
    } else {
        struct sockaddr_in *address = (struct sockaddr_in *)&storage;
        address->sin_family = AF_INET;
        address->sin_addr.s_addr = htonl(INADDR_LOOPBACK);
        length = sizeof(*address);
    }
    server = socket(family, SOCK_STREAM, 0);
    if (server < 0 || bind(server, (struct sockaddr *)&storage, length) != 0 ||
        listen(server, 1) != 0 || getsockname(server, (struct sockaddr *)&storage, &length) != 0) goto done;
    client = socket(family, SOCK_STREAM, 0);
    if (client < 0 || connect(client, (struct sockaddr *)&storage, length) != 0) goto done;
    peer = accept(server, NULL, NULL);
    char token[12];
    ok = peer >= 0 && send(client, "fixture-only", 12, MSG_NOSIGNAL) == 12 &&
        recv(peer, token, sizeof(token), MSG_WAITALL) == 12 && memcmp(token, "fixture-only", 12) == 0;
done:
    if (peer >= 0) close(peer);
    if (client >= 0) close(client);
    if (server >= 0) close(server);
    if (family == AF_UNIX) unlink(path);
    return ok;
}

static int pair_probe(const struct sockaddr_un *address, socklen_t length,
                      const char *data_path, const char *scratch_socket) {
    int pair[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, pair) != 0) return 3;
    char token[12];
    int roundtrip = send(pair[0], "fixture-only", 12, MSG_NOSIGNAL) == 12 &&
        recv(pair[1], token, sizeof(token), MSG_WAITALL) == 12 &&
        memcmp(token, "fixture-only", 12) == 0;
    int connected = connect(pair[0], (const struct sockaddr *)address, length) == 0;
    struct sockaddr unspecified = { .sa_family = AF_UNSPEC };
    int disconnected = connect(pair[0], &unspecified, sizeof(unspecified)) == 0;
    int reconnected = connect(pair[0], (const struct sockaddr *)address, length) == 0;
    int sent = (connected || reconnected) && send(pair[0], "fixture-only", 12, MSG_NOSIGNAL) == 12;
    struct sockaddr_un local = *address;
    size_t name_length = strlen(local.sun_path);
    if (name_length + 5 >= sizeof(local.sun_path)) return 2;
    memcpy(local.sun_path + name_length, "-pair", 6);
    int bound = bind(pair[0], (struct sockaddr *)&local,
        (socklen_t)(offsetof(struct sockaddr_un, sun_path) + name_length + 6)) == 0;
    int listening = listen(pair[0], 1) == 0;
    int sent_to = sendto(pair[0], "fixture-only", 12, MSG_NOSIGNAL,
        (const struct sockaddr *)address, length) == 12;
    struct iovec body = { .iov_base = (void *)"fixture-only", .iov_len = 12 };
    struct msghdr message = { .msg_name = (void *)address, .msg_namelen = length,
        .msg_iov = &body, .msg_iovlen = 1 };
    int sent_message = sendmsg(pair[0], &message, MSG_NOSIGNAL) == 12;
    close(pair[0]); close(pair[1]);
    int dgram[2], seqpacket[2];
    int datagram = socketpair(AF_UNIX, SOCK_DGRAM, 0, dgram) == 0;
    int sequence = socketpair(AF_UNIX, SOCK_SEQPACKET, 0, seqpacket) == 0;
    if (datagram) { close(dgram[0]); close(dgram[1]); }
    if (sequence) { close(seqpacket[0]); close(seqpacket[1]); }
    int data = open(data_path, O_RDONLY | O_CLOEXEC);
    char byte;
    int readable = data >= 0 && read(data, &byte, 1) == 1;
    if (data >= 0) close(data);
    int tcp = scratch_socket != NULL && local_exchange(AF_INET, NULL);
    int scratch_ipc = scratch_socket != NULL && local_exchange(AF_UNIX, scratch_socket);
    printf("{\"started\":true,\"connected\":%s,\"sent\":%s,\"project_read\":%s,"
        "\"pair_roundtrip\":%s,\"pair_disconnect\":%s,\"pair_reconnect\":%s,"
        "\"pair_bind\":%s,\"pair_listen\":%s,\"pair_sendto\":%s,\"pair_sendmsg\":%s,"
        "\"datagram_pair\":%s,\"seqpacket_pair\":%s,\"landlock_abi\":%d,"
        "\"tcp_roundtrip\":%s,\"scratch_unix_roundtrip\":%s}\n",
        (connected || reconnected) ? "true" : "false", sent ? "true" : "false",
        readable ? "true" : "false", roundtrip ? "true" : "false",
        disconnected ? "true" : "false", reconnected ? "true" : "false",
        bound ? "true" : "false", listening ? "true" : "false",
        sent_to ? "true" : "false", sent_message ? "true" : "false",
        datagram ? "true" : "false", sequence ? "true" : "false", abi(),
        tcp ? "true" : "false", scratch_ipc ? "true" : "false");
    return 0;
}

int main(int argc, char **argv) {
    if (argc != 3 && argc != 4 && argc != 5) return 2;
    struct sockaddr_un address = { .sun_family = AF_UNIX };
    size_t length = strlen(argv[1]);
    if (length == 0 || length >= sizeof(address.sun_path)) return 2;
    memcpy(address.sun_path, argv[1], length + 1);
    if (argc >= 4) {
        if (strcmp(argv[3], "pair") != 0) return 2;
        return pair_probe(&address,
            (socklen_t)(offsetof(struct sockaddr_un, sun_path) + length + 1), argv[2], argc == 5 ? argv[4] : NULL);
    }
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (fd >= 0 && fcntl(fd, F_SETFD, FD_CLOEXEC) != 0) {
        close(fd);
        fd = -1;
    }
    int connected = fd >= 0 && connect(fd, (struct sockaddr *)&address,
        (socklen_t)(offsetof(struct sockaddr_un, sun_path) + length + 1)) == 0;
    int error = connected ? 0 : errno;
    int sent = connected && send(fd, "fixture-only", 12, MSG_NOSIGNAL) == 12;
    if (fd >= 0) close(fd);
    int data = open(argv[2], O_RDONLY | O_CLOEXEC);
    char byte;
    int readable = data >= 0 && read(data, &byte, 1) == 1;
    if (data >= 0) close(data);
    printf("{\"started\":true,\"connected\":%s,\"sent\":%s,\"errno\":%d,\"project_read\":%s,\"landlock_abi\":%d}\n",
        connected ? "true" : "false", sent ? "true" : "false", error,
        readable ? "true" : "false", abi());
    return 0;
}
