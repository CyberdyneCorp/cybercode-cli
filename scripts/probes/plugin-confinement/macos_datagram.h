/* Fixed native datagram exchanges; requires the enclosing fixture's helpers. */
#ifndef CYBER_MACOS_DATAGRAM_H
#define CYBER_MACOS_DATAGRAM_H
#include <sys/uio.h>

static int private_datagram(const char *scratch, int method) {
    char server_path[PATH_MAX], client_path[PATH_MAX];
    if (!joined(server_path, scratch, "dgram-private.sock") || !joined(client_path, scratch, "dgram-client.sock")) return 0;
    int server = datagram_socket(server_path), client = datagram_socket(client_path), ok = 0;
    if (server < 0 || client < 0) goto done;
    struct unix_attempt sent = datagram_send(client, server_path, method);
    char token[12], ack = 0;
    struct sockaddr_un reply;
    ok = sent.sent && recv(server, token, sizeof(token), 0) == 12 && memcmp(token, "fixture-only", 12) == 0 &&
        datagram_address(client_path, &reply) && sendto(server, "A", 1, 0, (struct sockaddr *)&reply, sizeof(reply)) == 1 &&
        recv(client, &ack, 1, 0) == 1 && ack == 'A';
done:
    if (server >= 0) close(server);
    if (client >= 0) close(client);
    unlink(server_path); unlink(client_path);
    return ok;
}

static struct unix_attempt host_datagram(const char *scratch, const char *host, int method) {
    struct unix_attempt result = {0};
    char client_path[PATH_MAX];
    if (!joined(client_path, scratch, "dgram-client.sock")) return result;
    int client = datagram_socket(client_path);
    if (client < 0) return result;
    result = datagram_send(client, host, method);
    char ack = 0;
    result.acknowledged = result.sent && recv(client, &ack, 1, 0) == 1 && ack == 'A';
    close(client); unlink(client_path);
    return result;
}

#include "macos_datagram_race.h"

struct datagram_result { int private[3], symlink_created; struct unix_attempt host[6]; struct datagram_race_result race; };
static struct datagram_result datagram_probe(const char *scratch, const char *host) {
    struct datagram_result result = {0};
    char alias[PATH_MAX];
    if (!joined(alias, scratch, "dgram-host-link.sock")) return result;
    result.symlink_created = symlink(host, alias) == 0;
    for (int method = 0; method < 3; ++method) {
        result.private[method] = private_datagram(scratch, method);
        result.host[method] = host_datagram(scratch, host, method);
        if (result.symlink_created) result.host[method + 3] = host_datagram(scratch, alias, method);
    }
    unlink(alias);
    result.race = datagram_race(scratch, host);
    return result;
}

static void print_datagram(struct datagram_result result) {
    printf(",\"private_datagram\":[%s,%s,%s],\"datagram_symlink_created\":%s",
        truth(result.private[0]), truth(result.private[1]), truth(result.private[2]), truth(result.symlink_created));
    print_datagram_race(result.race);
    const char *names[6] = { "dgram_connect", "dgram_sendto", "dgram_sendmsg", "dgram_alias_connect", "dgram_alias_sendto", "dgram_alias_sendmsg" };
    for (int index = 0; index < 6; ++index) print_unix_attempt(names[index], result.host[index]);
}
#endif
