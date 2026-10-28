/* Test-only attempt to change a running image's cached signature without a
 * writable executable. The detached signature belongs to an equal-code copy. */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc == 2 && strcmp(argv[1], "--reexecuted") == 0) {
        puts("{\"self_exec\":true}");
        return 0;
    }
    if (argc != 2) return 2;
    int blobfd = open(argv[1], O_RDONLY | O_NOFOLLOW | O_CLOEXEC);
    struct stat info;
    if (blobfd < 0 || fstat(blobfd, &info) != 0 || !S_ISREG(info.st_mode) ||
        info.st_size <= 0 || info.st_size > 4 * 1024 * 1024) return 2;
    size_t size = (size_t)info.st_size, offset = 0;
    char *blob = malloc(size);
    if (blob == NULL) return 2;
    while (offset < size) {
        ssize_t count = read(blobfd, blob + offset, size - offset);
        if (count < 0 && errno == EINTR) continue;
        if (count <= 0) return 2;
        offset += (size_t)count;
    }
    close(blobfd);
    int self = open(argv[0], O_RDONLY | O_NOFOLLOW | O_CLOEXEC);
    if (self < 0) return 2;
    fsignatures_t signature = {0};
    signature.fs_blob_start = blob;
    signature.fs_blob_size = size;
    errno = 0;
    int result = fcntl(self, F_ADDSIGS, &signature);
    int saved_errno = errno;
    close(self);
    free(blob);
    printf("{\"initial_exec\":true,\"add_signature_result\":%d,\"add_signature_errno\":%d}\n",
        result, saved_errno);
    if (fflush(stdout) != 0) return 2;
    execl(argv[0], argv[0], "--reexecuted", (char *)NULL);
    saved_errno = errno;
    printf("{\"self_exec\":false,\"exec_errno\":%d}\n", saved_errno);
    return (saved_errno == EPERM || saved_errno == EACCES) ? 0 : 2;
}
