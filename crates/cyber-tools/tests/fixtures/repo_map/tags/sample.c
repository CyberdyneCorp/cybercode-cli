#include <stdio.h>
#define MAX_SIZE 64
#define SQUARE(x) ((x) * (x))

struct buffer {
    int size;
};

typedef struct buffer buffer_t;

int buffer_size(const struct buffer *b);

static int compute(int value) {
    return SQUARE(value);
}

char *name_of(int id) {
    return 0;
}
