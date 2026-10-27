/* Untrusted executable used only to isolate the initial-exec policy problem. */
#include <stdio.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc == 2) {
        puts("{\"initial_exec\":true,\"self_exec\":true}");
        return 0;
    }
    if (argc != 1) return 2;
    execl(argv[0], argv[0], "reexecuted", (char *)NULL);
    puts("{\"initial_exec\":true,\"self_exec\":false}");
    return 0;
}
