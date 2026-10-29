/* Trusted process controls; never loaded as plugin code. */
#include <stdio.h>
#include <unistd.h>
int main(void) {
#if CONTROL == 1
    FILE *receipt = fopen("alive", "w");
    if (!receipt) return 2;
    fprintf(receipt, "%d", getpid());
    fclose(receipt);
    for (;;) sleep(1);
#elif CONTROL == 2
    for (int i = 0; i < 65536; ++i) putchar('x');
#elif CONTROL == 3
    for (int i = 0; i < 16384; ++i) fputc('x', stderr);
#elif CONTROL == 4
    fputs("PRIVATE-WORKER-PAYLOAD", stderr);
    return 1;
#else
    fputs("{}", stdout);
#endif
    return 0;
}
