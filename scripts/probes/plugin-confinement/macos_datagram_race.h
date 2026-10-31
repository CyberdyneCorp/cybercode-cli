/* Concurrent replacement of an owned scratch alias; no host endpoint mutation. */
#ifndef CYBER_MACOS_DATAGRAM_RACE_H
#define CYBER_MACOS_DATAGRAM_RACE_H
#include <stdatomic.h>
#include <time.h>

struct datagram_mutator {
    const char *alias, *temporary, *private_path, *host;
    atomic_int stop, failed, error;
    atomic_uint swaps;
};

static void *replace_datagram_alias(void *value) {
    struct datagram_mutator *state = value;
    const struct timespec pause = { .tv_nsec = 100000 };
    for (unsigned i = 0; i < 4096 && !atomic_load(&state->stop); ++i) {
        const char *target = i % 2 ? state->private_path : state->host;
        if (symlink(target, state->temporary) != 0 || rename(state->temporary, state->alias) != 0) {
            atomic_store(&state->error, errno);
            atomic_store(&state->failed, 1); break;
        }
        atomic_fetch_add(&state->swaps, 1);
        nanosleep(&pause, NULL);
    }
    return NULL;
}

static int await_datagram_swap(struct datagram_mutator *state, unsigned previous) {
    struct timespec start, now;
    if (clock_gettime(CLOCK_MONOTONIC, &start) != 0) return 0;
    const struct timespec pause = { .tv_nsec = 50000 };
    while (atomic_load(&state->swaps) <= previous) {
        if (atomic_load(&state->failed) || clock_gettime(CLOCK_MONOTONIC, &now) != 0 || now.tv_sec - start.tv_sec >= 2) return 0;
        nanosleep(&pause, NULL);
    }
    return 1;
}

struct datagram_failure { const char *stage; int error; };

static char datagram_failed(struct datagram_failure *failure, const char *stage, int error) {
    if (!failure->stage) { failure->stage = stage; failure->error = error; }
    return 'E';
}

/* Returns P for an actual private acknowledgement, A for the owned host,
 * D for a policy refusal and E for any invalid/unknown exchange. */
static char datagram_race_exchange(int server, const char *scratch, const char *alias, int method,
                                   struct datagram_failure *failure) {
    char client_path[PATH_MAX], token[13], ack = 0;
    if (!joined(client_path, scratch, "dgram-client.sock")) return datagram_failed(failure, "client_path", ENAMETOOLONG);
    int client = datagram_socket(client_path);
    if (client < 0) return datagram_failed(failure, "client_bind", errno);
    struct unix_attempt sent = datagram_send(client, alias, method);
    int send_error = errno;
    if (!sent.sent) {
        ack = send_error == EACCES || send_error == EPERM ? 'D' : datagram_failed(failure, "send", send_error);
    } else {
        ssize_t size = recv(server, token, sizeof(token), MSG_DONTWAIT);
        if (size == 12 && memcmp(token, "fixture-only", 12) == 0) {
            struct sockaddr_un reply;
            if (!datagram_address(client_path, &reply)) {
                ack = datagram_failed(failure, "private_reply_path", ENAMETOOLONG); goto done;
            }
            if (sendto(server, "P", 1, 0, (struct sockaddr *)&reply, sizeof(reply)) != 1) {
                ack = datagram_failed(failure, "private_reply", errno); goto done;
            }
        } else if (size >= 0 || (errno != EAGAIN && errno != EWOULDBLOCK)) {
            ack = datagram_failed(failure, "private_receive", size < 0 ? errno : EPROTO); goto done;
        }
        ssize_t received = recv(client, &ack, 1, 0);
        if (received != 1 || (ack != 'P' && ack != 'A'))
            ack = datagram_failed(failure, "ack_receive", received < 0 ? errno : EPROTO);
    }
done:
    close(client); unlink(client_path);
    return ack ? ack : 'E';
}

struct datagram_race_result {
    int private_control, host_control, settled, failed, attempts, private_acks, host_acks, refused;
    unsigned swaps;
    struct datagram_failure failure;
};
static struct datagram_race_result datagram_race(const char *scratch, const char *host) {
    struct datagram_race_result result = {0};
    char server_path[PATH_MAX], alias[PATH_MAX], temporary[PATH_MAX];
    if (!joined(server_path, scratch, "dgram-race-private.sock") || !joined(alias, scratch, "dgram-race.sock") ||
        !joined(temporary, scratch, "dgram-race-next.sock")) {
        datagram_failed(&result.failure, "server_paths", ENAMETOOLONG); return result;
    }
    int server = datagram_socket(server_path);
    if (server < 0) { datagram_failed(&result.failure, "server_bind", errno); return result; }
    if (symlink(server_path, alias) != 0) { datagram_failed(&result.failure, "private_alias", errno); goto done; }
    result.private_control = datagram_race_exchange(server, scratch, alias, 1, &result.failure) == 'P';
    if (!result.private_control) goto done;
    struct datagram_mutator state = { .alias = alias, .temporary = temporary, .private_path = server_path, .host = host };
    atomic_init(&state.stop, 0); atomic_init(&state.failed, 0); atomic_init(&state.error, 0); atomic_init(&state.swaps, 0);
    pthread_t thread;
    int started = pthread_create(&thread, NULL, replace_datagram_alias, &state);
    if (started != 0) { datagram_failed(&result.failure, "mutator_start", started); goto done; }
    for (int i = 0; i < 16; ++i) {
        if (!await_datagram_swap(&state, atomic_load(&state.swaps))) {
            datagram_failed(&result.failure, atomic_load(&state.failed) ? "alias_replace" : "await_swap",
                            atomic_load(&state.failed) ? atomic_load(&state.error) : ETIMEDOUT);
            result.failed = 1; break;
        }
        char ack = datagram_race_exchange(server, scratch, alias, 1 + i % 2, &result.failure);
        result.attempts++;
        if (ack == 'P') result.private_acks++;
        else if (ack == 'A') result.host_acks++;
        else if (ack == 'D') result.refused++;
        else { result.failed = 1; break; }
    }
    atomic_store(&state.stop, 1);
    result.settled = pthread_join(thread, NULL) == 0;
    result.failed |= atomic_load(&state.failed);
    if (atomic_load(&state.failed)) datagram_failed(&result.failure, "alias_replace", atomic_load(&state.error));
    result.swaps = atomic_load(&state.swaps);
    if (!result.settled) _exit(2); /* Do not release stack/path ownership under a live thread. */
    if (symlink(host, temporary) != 0 || rename(temporary, alias) != 0) {
        datagram_failed(&result.failure, "host_alias", errno); result.failed = 1; goto done;
    }
    char ack = datagram_race_exchange(server, scratch, alias, 2, &result.failure);
    result.host_control = ack == 'A';
    if (ack != 'A' && ack != 'D') result.failed = 1;
done:
    close(server); unlink(server_path); unlink(alias); unlink(temporary);
    return result;
}

static void print_datagram_race(struct datagram_race_result result) {
    printf(",\"datagram_race\":{\"private_control\":%s,\"host_control\":%s,\"settled\":%s,\"failed\":%s,"
        "\"attempts\":%d,\"private_acks\":%d,\"host_acks\":%d,\"refused\":%d,\"swaps\":%u,"
        "\"failure_stage\":\"%s\",\"failure_errno\":%d}",
        truth(result.private_control), truth(result.host_control), truth(result.settled), truth(result.failed),
        result.attempts, result.private_acks, result.host_acks, result.refused, result.swaps,
        result.failure.stage ? result.failure.stage : "", result.failure.error);
}
#endif
