/* Native fixture compatibility check; no policy installation or production API. */
#ifndef CYBER_MACOS_RIGHTS_H
#define CYBER_MACOS_RIGHTS_H
#include <sys/uio.h>

struct rights_result {
    int sent, original_closed, token, truncated, received, same_inode, readable, written, cloexec;
};

static int rights_send(int channel, int descriptor) {
    union { struct cmsghdr alignment; char bytes[CMSG_SPACE(sizeof(int))]; } control = {0};
    struct iovec body = { .iov_base = (void *)"T", .iov_len = 1 };
    struct msghdr message = { .msg_iov = &body, .msg_iovlen = 1,
        .msg_control = control.bytes, .msg_controllen = sizeof(control.bytes) };
    struct cmsghdr *header = CMSG_FIRSTHDR(&message);
    header->cmsg_level = SOL_SOCKET;
    header->cmsg_type = SCM_RIGHTS;
    header->cmsg_len = CMSG_LEN(sizeof(int));
    memcpy(CMSG_DATA(header), &descriptor, sizeof(descriptor));
    return sendmsg(channel, &message, 0) == 1;
}

static int rights_receive(int channel, int drop, struct rights_result *result) {
    union { struct cmsghdr alignment; char bytes[CMSG_SPACE(sizeof(int))]; } control = {0};
    char token = 0;
    struct iovec body = { .iov_base = &token, .iov_len = 1 };
    struct msghdr message = { .msg_iov = &body, .msg_iovlen = 1,
        .msg_control = drop ? NULL : control.bytes,
        .msg_controllen = drop ? 0 : sizeof(control.bytes) };
    result->token = recvmsg(channel, &message, 0) == 1 && token == 'T';
    result->truncated = (message.msg_flags & MSG_CTRUNC) != 0;
    int descriptor = -1;
    struct cmsghdr *header = CMSG_FIRSTHDR(&message);
    if (header != NULL && header->cmsg_level == SOL_SOCKET && header->cmsg_type == SCM_RIGHTS &&
        header->cmsg_len == CMSG_LEN(sizeof(int))) memcpy(&descriptor, CMSG_DATA(header), sizeof(descriptor));
    if (!result->token || result->truncated || descriptor < 0 || CMSG_NXTHDR(&message, header) != NULL) {
        if (descriptor >= 0) close(descriptor);
        return -1;
    }
    return descriptor;
}

static struct rights_result private_rights(const char *scratch, int drop) {
    struct rights_result result = {0};
    char path[PATH_MAX];
    if (!joined(path, scratch, "rights.bin")) return result;
    int original = open(path, O_RDWR | O_NOFOLLOW), received = -1;
    int pair[2] = {-1, -1};
    struct stat before, after;
    if (original < 0 || write(original, "A", 1) != 1 || fstat(original, &before) != 0 ||
        lseek(original, 0, SEEK_SET) != 0 || socketpair(AF_UNIX, SOCK_STREAM, 0, pair) != 0) goto done;
    struct timeval timeout = { .tv_sec = 2 };
    if (setsockopt(pair[1], SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout)) != 0) goto done;
    result.sent = rights_send(pair[0], original);
    int old = original;
    if (close(original) != 0) { original = -1; goto done; }
    original = -1;
    result.original_closed = fcntl(old, F_GETFD) < 0 && errno == EBADF;
    if (!result.sent) goto done;
    received = rights_receive(pair[1], drop, &result);
    result.received = received >= 0;
    if (received < 0) goto done;
    result.same_inode = fstat(received, &after) == 0 && S_ISREG(after.st_mode) &&
        before.st_dev == after.st_dev && before.st_ino == after.st_ino;
    char byte = 0, contents[2] = {0};
    result.readable = read(received, &byte, 1) == 1 && byte == 'A';
    result.written = result.readable && write(received, "B", 1) == 1 && lseek(received, 0, SEEK_SET) == 0 &&
        read(received, contents, sizeof(contents)) == 2 && memcmp(contents, "AB", 2) == 0;
    int set_flags = fcntl(received, F_SETFD, FD_CLOEXEC), flags = fcntl(received, F_GETFD);
    result.cloexec = set_flags == 0 && flags >= 0 && (flags & FD_CLOEXEC) != 0;
done:
    if (original >= 0) close(original);
    if (received >= 0) close(received);
    if (pair[0] >= 0) close(pair[0]);
    if (pair[1] >= 0) close(pair[1]);
    return result;
}

static int replace_rights_path(const char *scratch) {
    char original[PATH_MAX], replacement[PATH_MAX];
    if (!joined(original, scratch, "rights.bin") || !joined(replacement, scratch, "rights-fake.bin")) return 0;
    int fd = open(replacement, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW, 0600);
    if (fd < 0) return 0;
    int written = write(fd, "XX", 2) == 2;
    close(fd);
    return written && unlink(original) == 0 && symlink("rights-fake.bin", original) == 0;
}

static void print_rights(struct rights_result result) {
    printf(",\"rights\":{\"sent\":%s,\"original_closed\":%s,\"token\":%s,\"truncated\":%s,"
        "\"received\":%s,\"same_inode\":%s,\"readable\":%s,\"written\":%s,\"cloexec\":%s}",
        truth(result.sent), truth(result.original_closed), truth(result.token), truth(result.truncated),
        truth(result.received), truth(result.same_inode), truth(result.readable), truth(result.written), truth(result.cloexec));
}
#endif
