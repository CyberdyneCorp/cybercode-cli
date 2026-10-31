/* Untrusted native executable fixture. No policy installation or cooperation:
 * even its ELF/Mach-O constructor enters with its read/process policy installed. */
#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <pthread.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <unistd.h>
#include "process_policy.h"
#ifdef __APPLE__
#include <crt_externs.h>
#include <stdint.h>
/* XNU bsd/sys/codesign.h: CS_OPS_STATUS=0, CS_OPS_SET_STATUS=9.
 * Query and attempt to assert platform status; no successful setter return is
 * mistaken for authority, because XNU masks the accepted status bits. */
extern int csops(pid_t, unsigned int, void *, size_t);
#define PROOF_CS_PLATFORM_BINARY 0x04000000u
static int platform_status_known, platform_before, platform_after_set, constructor_spawn;
#endif

static const char *truth(int value) { return value ? "true" : "false"; }
static int native_read(const char *path) {
    if (path == NULL) return 0;
    int fd = open(path, O_RDONLY);
    char byte;
    int result = fd >= 0 && read(fd, &byte, 1) == 1;
    if (fd >= 0) close(fd);
    return result;
}

static int native_fork(void);
static int native_spawn(const char *executable);
extern char **environ;
static int constructor_read, constructor_fork;
__attribute__((constructor)) static void untrusted_constructor(void) {
#ifdef __APPLE__
    uint32_t before = 0, after = 0, request = PROOF_CS_PLATFORM_BINARY;
    platform_status_known = csops(0, 0, &before, sizeof(before)) == 0;
    platform_before = (before & PROOF_CS_PLATFORM_BINARY) != 0;
    (void)csops(0, 9, &request, sizeof(request));
    platform_status_known &= csops(0, 0, &after, sizeof(after)) == 0;
    platform_after_set = (after & PROOF_CS_PLATFORM_BINARY) != 0;
    char **args = *_NSGetArgv();
    if (*_NSGetArgc() > 1 && strcmp(args[1], "ctor-exec") == 0) {
        execl(args[0], args[0], "receipt", (char *)NULL);
    }
    if (*_NSGetArgc() > 1 && strcmp(args[1], "ctor-spawn") == 0) {
        constructor_spawn = native_spawn(args[0]);
    }
#endif
    constructor_read = native_read(getenv("CYBER_PROOF_PRIVATE"));
    constructor_fork = native_fork();
}

static int native_write(const char *path) {
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0600);
    int result = fd >= 0 && write(fd, "fixture only", 12) == 12;
    if (fd >= 0) close(fd);
    return result;
}

static int list(const char *path) {
    DIR *directory = opendir(path);
    if (directory == NULL) return 0;
    closedir(directory);
    return 1;
}

static int joined(char *out, const char *directory, const char *name) {
    int size = snprintf(out, PATH_MAX, "%s/%s", directory, name);
    return size > 0 && size < PATH_MAX;
}

#ifdef __APPLE__
static int private_peer_checks, private_peer_matches = 1;
static void private_peer_identity(int fd) {
    uid_t uid = 0; gid_t gid = 0; pid_t pid = 0;
    socklen_t size = sizeof(pid);
    int match = getpeereid(fd, &uid, &gid) == 0 && uid == getuid() && gid == getgid() &&
        getsockopt(fd, SOL_LOCAL, LOCAL_PEERPID, &pid, &size) == 0 &&
        size == sizeof(pid) && pid == getpid();
    private_peer_checks++;
    private_peer_matches = private_peer_matches && match;
}
#endif

static int private_stream_exchange_to(const char *path, const char *destination, int alias_type) {
    struct sockaddr_un address = { .sun_family = AF_UNIX };
    if (strlen(path) >= sizeof(address.sun_path) ||
        (destination != NULL && strlen(destination) >= sizeof(address.sun_path))) return 0;
    strcpy(address.sun_path, path);
    int server = socket(AF_UNIX, SOCK_STREAM, 0), client = -1, peer = -1;
    int bound = 0, aliased = 0, result = 0;
    struct timeval timeout = { .tv_sec = 2 };
    if (server < 0 || setsockopt(server, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout)) != 0 ||
        bind(server, (struct sockaddr *)&address, sizeof(address)) != 0) goto done;
    bound = 1;
    if (listen(server, 1) != 0) goto done;
    if (destination != NULL) {
        int result = alias_type == 1 ? link(path, destination) : symlink(path, destination);
        if (result != 0) goto done;
        aliased = 1;
        strcpy(address.sun_path, destination);
    }
    client = socket(AF_UNIX, SOCK_STREAM, 0);
    if (client < 0 || setsockopt(client, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout)) != 0 ||
        setsockopt(client, SOL_SOCKET, SO_SNDTIMEO, &timeout, sizeof(timeout)) != 0 ||
        connect(client, (struct sockaddr *)&address, sizeof(address)) != 0) goto done;
    peer = accept(server, NULL, NULL);
    if (peer < 0 || setsockopt(peer, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout)) != 0) goto done;
    char token = 0, ack = 0;
    result = send(client, "T", 1, 0) == 1 && recv(peer, &token, 1, 0) == 1 && token == 'T' &&
        send(peer, "A", 1, 0) == 1 && recv(client, &ack, 1, 0) == 1 && ack == 'A';
#ifdef __APPLE__
    if (result) { private_peer_identity(client); private_peer_identity(peer); }
#endif
done:
    if (peer >= 0) close(peer);
    if (client >= 0) close(client);
    if (server >= 0) close(server);
    if (aliased) unlink(destination);
    if (bound) unlink(path);
    return result;
}

static int private_stream_exchange(const char *path) {
    return private_stream_exchange_to(path, NULL, 0);
}

static int private_stream_probe(const char *scratch, int result[2]) {
    char absolute[PATH_MAX];
    if (!joined(absolute, scratch, "a.sock")) return 0;
    result[0] = private_stream_exchange(absolute);
    int previous = open(".", O_RDONLY | O_DIRECTORY);
    if (previous < 0) return 0;
    int restored = 1;
    if (chdir(scratch) == 0) {
        result[1] = private_stream_exchange("r.sock");
        restored = fchdir(previous) == 0;
    }
    close(previous);
    return restored;
}

#ifdef __APPLE__
#include "macos_rights.h"
struct unix_attempt {
    int connected, sent, acknowledged;
    /* Datagram diagnostics retain errno only for a real failed send. */
    int attempted, error;
    ssize_t returned;
};
static struct unix_attempt host_unix_attempt(const char *path) {
    struct unix_attempt result = {0};
    struct sockaddr_un address = { .sun_family = AF_UNIX };
    if (strlen(path) >= sizeof(address.sun_path)) return result;
    strcpy(address.sun_path, path);
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (fd < 0) return result;
    struct timeval timeout = { .tv_sec = 2 };
    if (setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout)) != 0 ||
        setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &timeout, sizeof(timeout)) != 0) goto done;
    result.connected = connect(fd, (struct sockaddr *)&address, sizeof(address)) == 0;
    result.sent = result.connected && send(fd, "fixture-only", 12, 0) == 12;
    char ack = 0;
    result.acknowledged = result.sent && recv(fd, &ack, 1, 0) == 1 && ack == 'A';
done:
    close(fd);
    return result;
}

static void print_unix_attempt(const char *name, struct unix_attempt result) {
    printf(",\"%s\":{\"connected\":%s,\"sent\":%s,\"acknowledged\":%s}", name,
        truth(result.connected), truth(result.sent), truth(result.acknowledged));
}

#include "macos_datagram.h"

static int unix_boundary(const char *scratch, const char *host, const char *datagram_host) {
    if (strlen(scratch) >= PATH_MAX || strlen(host) >= PATH_MAX) return 2;
    int ipc[2] = {0};
    if (!private_stream_probe(scratch, ipc)) return 2;
    char owned[PATH_MAX], owned_alias[PATH_MAX];
    if (!joined(owned, scratch, "owned.sock") || !joined(owned_alias, scratch, "owned-alias.sock")) return 2;
    int own_hardlink = private_stream_exchange_to(owned, owned_alias, 1);
    int own_symlink = private_stream_exchange_to(owned, owned_alias, 2);
    const char *drop = getenv("CYBER_PROOF_DROP_RIGHTS");
    struct rights_result rights = private_rights(scratch, drop != NULL && strcmp(drop, "1") == 0);
    int rights_path_replaced = replace_rights_path(scratch);
    struct datagram_result datagram = datagram_probe(scratch, datagram_host);
    struct unix_attempt direct = host_unix_attempt(host);
    char alias[PATH_MAX], symbolic[PATH_MAX];
    if (!joined(alias, scratch, "host-alias.sock") || !joined(symbolic, scratch, "host-symlink.sock")) return 2;
    int linked = link(host, alias) == 0;
    struct unix_attempt hardlink = linked ? host_unix_attempt(alias) : (struct unix_attempt){0};
    int symlinked = symlink(host, symbolic) == 0;
    struct unix_attempt symlink_result = symlinked ? host_unix_attempt(symbolic) : (struct unix_attempt){0};
    printf("{\"started\":true,\"pid\":%ld,\"private_unix\":[%s,%s],\"private_peer_checks\":%d,"
        "\"private_peer_matches\":%s,\"hardlink_created\":%s,\"symlink_created\":%s",
        (long)getpid(), truth(ipc[0]), truth(ipc[1]), private_peer_checks, truth(private_peer_matches), truth(linked), truth(symlinked));
    printf(",\"private_aliases\":[%s,%s]", truth(own_hardlink), truth(own_symlink));
    print_rights(rights);
    printf(",\"rights_path_replaced\":%s", truth(rights_path_replaced));
    print_datagram(datagram);
    print_unix_attempt("direct", direct);
    print_unix_attempt("hardlink", hardlink);
    print_unix_attempt("symlink", symlink_result);
    puts("}");
    return 0;
}
#endif

/* No untrusted shell strings: only fixed commands and separate fixture paths. */
static int child(char *const args[], char *output, size_t size) {
    int channel[2];
    if (pipe(channel) != 0) return -2;
    if (fcntl(channel[0], F_SETFD, FD_CLOEXEC) != 0 || fcntl(channel[1], F_SETFD, FD_CLOEXEC) != 0) {
        close(channel[0]); close(channel[1]); return -2;
    }
    pid_t pid = fork();
    if (pid < 0) { close(channel[0]); close(channel[1]); return -1; }
    if (pid == 0) {
        close(channel[0]);
        int quiet = open("/dev/null", O_RDWR);
        if (quiet < 0 || dup2(output ? channel[1] : quiet, STDOUT_FILENO) < 0 ||
            dup2(quiet, STDERR_FILENO) < 0) _exit(126);
        close(channel[1]);
        if (quiet > STDERR_FILENO) close(quiet);
        execv(args[0], args);
        _exit(127);
    }
    close(channel[1]);
    if (output != NULL) {
        ssize_t count;
        do { count = read(channel[0], output, size - 1); } while (count < 0 && errno == EINTR);
        if (count < 0) { close(channel[0]); return -2; }
        output[count] = '\0';
    }
    close(channel[0]);
    int status;
    pid_t waited;
    do { waited = waitpid(pid, &status, 0); } while (waited < 0 && errno == EINTR);
    return waited == pid && WIFEXITED(status) ? WEXITSTATUS(status) : -2;
}

static int shell(void) {
    char *const args[] = {"/bin/sh", "-c", "exit 0", NULL};
    return child(args, NULL, 0) == 0;
}

static int native_fork(void) {
    pid_t pid = fork();
    if (pid < 0) return 0;
    if (pid == 0) _exit(0);
    int status;
    pid_t waited;
    do { waited = waitpid(pid, &status, 0); } while (waited < 0 && errno == EINTR);
    return waited == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0;
}

/* posix_spawn may use a different kernel path than fork+exec. Exercise it
 * directly against the same staged image, including the literal exec exception. */
static int native_spawn(const char *executable) {
    posix_spawn_file_actions_t actions;
    if (posix_spawn_file_actions_init(&actions) != 0) return 0;
    if (posix_spawn_file_actions_addopen(&actions, STDOUT_FILENO, "/dev/null", O_WRONLY, 0) != 0 ||
        posix_spawn_file_actions_addopen(&actions, STDERR_FILENO, "/dev/null", O_WRONLY, 0) != 0) {
        posix_spawn_file_actions_destroy(&actions); return 0;
    }
    char *const args[] = {(char *)executable, "receipt", NULL};
    pid_t pid;
    int result = posix_spawn(&pid, executable, &actions, NULL, args, environ);
    posix_spawn_file_actions_destroy(&actions);
    if (result != 0) return 0;
    int status;
    pid_t waited;
    do { waited = waitpid(pid, &status, 0); } while (waited < 0 && errno == EINTR);
    return waited == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0;
}

struct worker { const char *project, *private; int project_read, private_read, shell; };
static void *thread_work(void *value) {
    struct worker *worker = value;
    worker->project_read = native_read(worker->project);
    worker->private_read = native_read(worker->private);
    worker->shell = shell();
    return NULL;
}

static int parent_directory(const char *path, char *out) {
    if (strlen(path) >= PATH_MAX) return 0;
    strcpy(out, path);
    char *slash = strrchr(out, '/');
    if (slash == NULL) return 0;
    *slash = '\0';
    return 1;
}

int main(int argc, char **argv) {
#ifdef __APPLE__
    if (argc == 5 && strcmp(argv[1], "unix-boundary") == 0) return unix_boundary(argv[2], argv[3], argv[4]);
#endif
    if (argc == 2 && strcmp(argv[1], "receipt") == 0) {
        puts("{\"self_exec\":true}");
        return 0;
    }
    if (argc == 4 && strcmp(argv[1], "read") == 0) {
        printf("[%s,%s]", truth(native_read(argv[2])), truth(native_read(argv[3])));
        return 0;
    }
    if (argc != 14) return 2;
    for (int index = 2; index <= 10; index++) if (strlen(argv[index]) >= PATH_MAX) return 2;
#ifdef __APPLE__
    if (strcmp(argv[1], "ctor-spawn") == 0) {
        printf("{\"native_spawn\":%s}\n", truth(constructor_spawn));
        return 0;
    }
#endif
    if (strcmp(argv[1], "ctor-exec") == 0) {
        puts("{\"self_exec\":false}");
        return 0;
    }
    if (strcmp(argv[1], "exec") == 0) {
        execl(argv[0], argv[0], "receipt", (char *)NULL);
        puts("{\"self_exec\":false}");
        return 0;
    }
    if (strcmp(argv[1], "inspect") != 0) return 2;
    char packet[4097];
    const char *request = "{\"jsonrpc\":\"2.0\",\"id\":\"native-proof\",\"method\":\"tool/execute\",\"params\":{\"name\":\"native-probe\",\"input\":{}}}\n";
    if (fgets(packet, sizeof(packet), stdin) == NULL || strcmp(packet, request) != 0) return 2;
    const char *package = argv[2], *project = argv[3], *private = argv[4], *scratch = argv[5];
    const char *written = argv[6], *symlink = argv[7], *protected = argv[8];
    const char *scratch_dir = argv[9], *project_dir = argv[10];
    struct stat identity;
    int inherited = fstat(9, &identity) == 0 &&
        (unsigned long long)identity.st_dev == strtoull(argv[11], NULL, 10) &&
        (unsigned long long)identity.st_ino == strtoull(argv[12], NULL, 10);
    struct worker worker = { .project = project, .private = private };
    pthread_t thread;
    if (pthread_create(&thread, NULL, thread_work, &worker) != 0 || pthread_join(thread, NULL) != 0) return 2;
    char private_dir[PATH_MAX], alias[PATH_MAX], moved[PATH_MAX], package_alias[PATH_MAX];
    if (!parent_directory(private, private_dir) || !joined(alias, scratch_dir, "private-alias") ||
        !joined(moved, scratch_dir, "moved-project") ||
        !joined(package_alias, scratch_dir, "package-alias")) return 2;
    int hardlink = link(private, alias) == 0 && native_read(alias);
    /* Package data is readable, unlike the private-file control. Linking it into
     * writable scratch must not manufacture a write alias to a read-only root. */
    int package_link = link(package, package_alias) == 0;
    int package_alias_write = package_link && native_write(package_alias);
    int renamed = rename(project_dir, moved) == 0;
    if (renamed && rename(moved, project_dir) != 0) return 2;
    int scratch_write = native_write(scratch), project_write = native_write(written);
    char child_result[64] = {0};
    char *const native_args[] = {argv[0], "read", argv[3], argv[4], NULL};
    int native_status = child(native_args, child_result, sizeof(child_result));
    if (native_status != -1 && native_status != 0) return 2;
    char *const cat_project[] = {"/bin/cat", argv[3], NULL};
    char *const cat_private[] = {"/bin/cat", argv[4], NULL};
    int cat_read = child(cat_project, NULL, 0), cat_secret = child(cat_private, NULL, 0);
    if (cat_read == -2 || cat_secret == -2) return 2;
    char native_children[64] = "null";
    if (cat_read != -1 || cat_secret != -1) {
        snprintf(native_children, sizeof(native_children), "[%s,%s]", truth(cat_read == 0), truth(cat_secret == 0));
    }
    int confined = strcmp(argv[13], "1") == 0;
    const char *private_stream = getenv("CYBER_PROOF_PRIVATE_STREAM");
    int probe_ipc = private_stream != NULL && strcmp(private_stream, "1") == 0;
    int ipc[2] = {0};
    if (probe_ipc && !private_stream_probe(scratch_dir, ipc)) return 2;
    printf("{\"jsonrpc\":\"2.0\",\"id\":\"native-proof\",\"result\":{\"stdio_tool\":true,\"package_read\":%s,\"scratch_write\":%s,\"scratch_read\":%s,",
           truth(native_read(package)), truth(scratch_write), truth(native_read(scratch)));
    printf("\"project_read\":%s,\"project_list\":%s,\"project_write\":%s,\"written_project_read\":%s,",
           truth(native_read(project)), truth(list(project_dir)), truth(project_write), truth(native_read(written)));
    printf("\"private_read\":%s,\"private_list\":%s,\"private_write\":%s,",
           truth(native_read(private)), truth(list(private_dir)), truth(native_write(private)));
    printf("\"protected_read\":%s,\"protected_write\":%s,\"symlink_private_read\":%s,",
           truth(native_read(protected)), truth(native_write(protected)), truth(native_read(symlink)));
    printf("\"hardlink_private\":%s,\"project_rename\":%s,\"inherited_descriptor\":%s,\"thread\":true,",
           truth(hardlink), truth(renamed), truth(inherited));
    printf("\"package_hardlink\":%s,\"package_alias_write\":%s,", truth(package_link), truth(package_alias_write));
    printf("\"thread_reads\":[%s,%s],\"thread_shell\":%s,\"shell\":%s,\"native_fork\":%s,",
           truth(worker.project_read), truth(worker.private_read), truth(worker.shell), truth(shell()), truth(native_fork()));
    printf("\"child_reads\":%s,\"native_child_reads\":%s,\"cannot_relax\":%s,",
           native_status == -1 ? "null" : child_result, native_children,
           confined ? truth(proof_relax() != 0 && !native_read(private)) : "null");
    printf("\"constructor_private_read\":%s,\"constructor_fork\":%s,\"native_spawn\":%s",
           truth(constructor_read), truth(constructor_fork), truth(native_spawn(argv[0])));
#ifdef __APPLE__
    printf(",\"platform_status_known\":%s,\"platform_before\":%s,\"platform_after_set\":%s",
           truth(platform_status_known), truth(platform_before), truth(platform_after_set));
#endif
#ifdef __linux__
    printf(",\"filter_replacement_denied\":%s", truth(proof_replacement_denied()));
#endif
    if (probe_ipc) printf(",\"private_unix\":[%s,%s]", truth(ipc[0]), truth(ipc[1]));
    puts("}}");
    return 0;
}
