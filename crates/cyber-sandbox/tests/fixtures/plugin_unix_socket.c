/* Fixture-only connection probe. It never contacts a non-fixture service. */
#include <errno.h>
#include <fcntl.h>
#include <stddef.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc != 3) return 2;
    struct sockaddr_un address = { .sun_family = AF_UNIX };
    size_t length = strlen(argv[1]);
    if (length == 0 || length >= sizeof(address.sun_path)) return 2;
    memcpy(address.sun_path, argv[1], length + 1);
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
    printf("{\"started\":true,\"connected\":%s,\"sent\":%s,\"errno\":%d,\"project_read\":%s}\n",
        connected ? "true" : "false", sent ? "true" : "false", error,
        readable ? "true" : "false");
    return 0;
}
