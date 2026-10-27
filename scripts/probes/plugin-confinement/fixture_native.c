/* Untrusted Linux executable fixture. No policy installation or cooperation:
 * even this ELF constructor must enter with its read/process policy installed. */
#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>
#include "process_policy.h"

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
static int constructor_read, constructor_fork;
__attribute__((constructor)) static void untrusted_constructor(void) {
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
    char private_dir[PATH_MAX], alias[PATH_MAX], moved[PATH_MAX];
    if (!parent_directory(private, private_dir) || !joined(alias, scratch_dir, "private-alias") ||
        !joined(moved, scratch_dir, "moved-project")) return 2;
    int hardlink = link(private, alias) == 0 && native_read(alias);
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
    printf("\"thread_reads\":[%s,%s],\"thread_shell\":%s,\"shell\":%s,\"native_fork\":%s,",
           truth(worker.project_read), truth(worker.private_read), truth(worker.shell), truth(shell()), truth(native_fork()));
    printf("\"child_reads\":%s,\"native_child_reads\":%s,\"cannot_relax\":%s,",
           native_status == -1 ? "null" : child_result, native_children,
           confined ? truth(proof_relax() != 0 && !native_read(private)) : "null");
    printf("\"constructor_private_read\":%s,\"constructor_fork\":%s",
           truth(constructor_read), truth(constructor_fork));
#ifdef __linux__
    printf(",\"filter_replacement_denied\":%s", truth(proof_replacement_denied()));
#endif
    puts("}}");
    return 0;
}
