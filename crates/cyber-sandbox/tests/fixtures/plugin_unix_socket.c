/* Fixture-only connection probe. It never contacts a non-fixture service. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stddef.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <sys/uio.h>
#include <unistd.h>
#include <netinet/in.h>
#include <pthread.h>
#ifdef __linux__
#include <linux/close_range.h>
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

static int local_exchange(int family, const char *path, int server) {
    int client = -1, peer = -1, ok = 0;
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
    if (server < 0) server = socket(family, SOCK_STREAM, 0);
    if (server < 0 || bind(server, (struct sockaddr *)&storage, length) != 0 ||
        listen(server, 1) != 0 || getsockname(server, (struct sockaddr *)&storage, &length) != 0) goto done;
    client = socket(family, SOCK_STREAM, 0);
    if (client < 0 || connect(client, (struct sockaddr *)&storage, length) != 0) goto done;
    peer = accept(server, NULL, NULL);
    char token[12], ack = 0;
    ok = peer >= 0 && send(client, "fixture-only", 12, MSG_NOSIGNAL) == 12 &&
        recv(peer, token, sizeof(token), MSG_WAITALL) == 12 && memcmp(token, "fixture-only", 12) == 0 &&
        send(peer, "A", 1, MSG_NOSIGNAL) == 1 && recv(client, &ack, 1, MSG_WAITALL) == 1 && ack == 'A';
done:
    if (peer >= 0) close(peer);
    if (client >= 0) close(client);
    if (server >= 0) close(server);
    if (family == AF_UNIX) unlink(path);
    return ok;
}

#include "plugin_unix_relative.h"

#ifdef __linux__
struct thread_exchange { int descriptor; int roundtrip; };
static void *thread_exchange(void *argument) {
    struct thread_exchange *test = argument;
    /* A real runtime may detach its FD table without unshare(2). Leave the
     * leader's UNIX socket at this number while this thread installs an IP
     * socket there. close_range UNSHARE closes only this thread's unused stdin. */
    if (syscall(SYS_close_range, 0U, 0U, CLOSE_RANGE_UNSHARE) != 0) return NULL;
    int fd = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (fd < 0) return NULL;
    if (dup2(fd, test->descriptor) != test->descriptor) { close(fd); return NULL; }
    if (fd != test->descriptor) close(fd);
    test->roundtrip = local_exchange(AF_INET, NULL, test->descriptor);
    return NULL;
}

static int thread_local_exchange(void) {
    int pair[2];
    if (socketpair(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0, pair) != 0) return 0;
    int descriptor = fcntl(pair[0], F_DUPFD_CLOEXEC, 128);
    close(pair[0]);
    if (descriptor < 0) { close(pair[1]); return 0; }
    struct thread_exchange test = { .descriptor = descriptor };
    pthread_t thread;
    int ok = pthread_create(&thread, NULL, thread_exchange, &test) == 0;
    if (ok && pthread_join(thread, NULL) != 0) _exit(3);
    int domain = 0;
    socklen_t length = sizeof(domain);
    ok = ok && test.roundtrip && getsockopt(descriptor, SOL_SOCKET, SO_DOMAIN, &domain, &length) == 0 &&
        length == sizeof(domain) && domain == AF_UNIX;
    close(descriptor); close(pair[1]);
    return ok;
}
#else
static int thread_local_exchange(void) { return 0; }
#endif

static int regular_reparent(const char *data_path) {
    char first[512], second[512], source[544], target[544];
    int a = snprintf(first, sizeof(first), "%s-move-a", data_path);
    int b = snprintf(second, sizeof(second), "%s-move-b", data_path);
    if (a <= 0 || b <= 0 || (size_t)a >= sizeof(first) || (size_t)b >= sizeof(second)) return 0;
    snprintf(source, sizeof(source), "%s/entry", first);
    snprintf(target, sizeof(target), "%s/entry", second);
    int ok = 0, fd = -1;
    struct stat before, after, old;
    if (mkdir(first, 0700) != 0 || mkdir(second, 0700) != 0) goto done;
    fd = open(source, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0600);
    if (fd < 0 || write(fd, "T", 1) != 1 || fstat(fd, &before) != 0) goto done;
    close(fd); fd = -1;
    ok = rename(source, target) == 0 && lstat(target, &after) == 0 &&
        S_ISREG(after.st_mode) && before.st_dev == after.st_dev && before.st_ino == after.st_ino &&
        lstat(source, &old) < 0 && errno == ENOENT;
done:
    if (fd >= 0) close(fd);
    unlink(source); unlink(target); rmdir(first); rmdir(second);
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
    int tcp = scratch_socket != NULL && local_exchange(AF_INET, NULL, -1);
    int thread_tcp = scratch_socket != NULL && thread_local_exchange();
    int scratch_ipc = scratch_socket != NULL && local_exchange(AF_UNIX, scratch_socket, -1);
    struct relative_result relative = relative_probe(scratch_socket);
    int moved = regular_reparent(data_path);
    int alias_error = 0;
    if (scratch_socket != NULL) {
        char alias[512];
        int size = snprintf(alias, sizeof(alias), "%s-alias", scratch_socket);
        if (size <= 0 || (size_t)size >= sizeof(alias)) return 2;
        if (link(address->sun_path, alias) == 0) unlink(alias);
        else alias_error = errno;
    }
    printf("{\"started\":true,\"connected\":%s,\"sent\":%s,\"project_read\":%s,"
        "\"pair_roundtrip\":%s,\"pair_disconnect\":%s,\"pair_reconnect\":%s,"
        "\"pair_bind\":%s,\"pair_listen\":%s,\"pair_sendto\":%s,\"pair_sendmsg\":%s,"
        "\"datagram_pair\":%s,\"seqpacket_pair\":%s,\"landlock_abi\":%d,"
        "\"tcp_roundtrip\":%s,\"thread_tcp_roundtrip\":%s,\"scratch_unix_roundtrip\":%s,\"regular_reparent\":%s,"
        "\"cross_mount_alias_errno\":%d,\"relative_root_roundtrip\":%s,\"relative_parent_roundtrip\":%s,"
        "\"relative_escape_connected\":%s,\"relative_escape_sent\":%s}\n",
        (connected || reconnected) ? "true" : "false", sent ? "true" : "false",
        readable ? "true" : "false", roundtrip ? "true" : "false",
        disconnected ? "true" : "false", reconnected ? "true" : "false",
        bound ? "true" : "false", listening ? "true" : "false",
        sent_to ? "true" : "false", sent_message ? "true" : "false",
        datagram ? "true" : "false", sequence ? "true" : "false", abi(),
        tcp ? "true" : "false", thread_tcp ? "true" : "false", scratch_ipc ? "true" : "false",
        moved ? "true" : "false", alias_error,
        relative.root ? "true" : "false", relative.parent ? "true" : "false",
        relative.escape_connected ? "true" : "false", relative.escape_sent ? "true" : "false");
    return 0;
}

#include "plugin_socket_cancel.h"
#include "plugin_socket_saturation.h"
#include "plugin_socket_mutation.h"
#include "plugin_unix_mutation.h"
#include "plugin_process_escape.h"

int main(int argc, char **argv) {
    if (argc != 3 && argc != 4 && argc != 5) return 2;
    struct sockaddr_un address = { .sun_family = AF_UNIX };
    size_t length = strlen(argv[1]);
    if (length == 0 || length >= sizeof(address.sun_path)) return 2;
    memcpy(address.sun_path, argv[1], length + 1);
    if (argc == 4 && strcmp(argv[3], "cancel") == 0) return cancel_probe();
    if (argc == 4 && strcmp(argv[3], "mutation") == 0) return mutation_probe();
    if (argc == 4 && strcmp(argv[3], "escape") == 0) return escape_probe();
    if (argc == 4 && strcmp(argv[3], "saturation") == 0) return saturation_probe();
    if (argc == 5 && strcmp(argv[3], "unix-mutation") == 0) return unix_mutation_probe(argv[1], argv[4]);
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
