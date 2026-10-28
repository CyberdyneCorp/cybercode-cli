/* Dummy-file admission control; no policy installation and no owner data. */
#include <fcntl.h>
#include <stdio.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc != 4) return 2;
    int receipt = open(argv[3], O_WRONLY | O_CREAT | O_TRUNC, 0600);
    if (receipt < 0 || write(receipt, "started", 7) != 7) return 2;
    close(receipt);
    int fd = open(argv[1], O_RDONLY);
    char byte;
    int read_alias = fd >= 0 && read(fd, &byte, 1) == 1;
    if (fd >= 0) close(fd);
    fd = open(argv[2], O_WRONLY | O_TRUNC);
    int write_alias = fd >= 0 && write(fd, "alias mutation", 14) == 14;
    if (fd >= 0) close(fd);
    printf("{\"initial_exec\":true,\"private_alias_read\":%s,\"package_alias_write\":%s}\n",
        read_alias ? "true" : "false", write_alias ? "true" : "false");
    return 0;
}
