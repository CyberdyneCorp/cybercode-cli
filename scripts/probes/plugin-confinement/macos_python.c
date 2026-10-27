/* Experimental native boundary proof, not a production plugin launcher.
 * Initialize only an explicitly selected isolated Python, install Seatbelt in
 * this native process, then execute the fixture. No Python API is monkeypatched.
 */
#include <Python.h>
#include <errno.h>
#include <fcntl.h>
#include <sandbox.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static char *profile_text(const char *path) {
    int fd = open(path, O_RDONLY | O_NOFOLLOW | O_CLOEXEC);
    struct stat info;
    if (fd < 0 || fstat(fd, &info) != 0 || !S_ISREG(info.st_mode) ||
        info.st_nlink != 1 || info.st_size <= 0 || info.st_size > 65536) {
        if (fd >= 0) close(fd);
        return NULL;
    }
    size_t size = (size_t)info.st_size;
    char *text = calloc(size + 1, 1);
    if (text == NULL) { close(fd); return NULL; }
    size_t offset = 0;
    while (offset < size) {
        ssize_t count = read(fd, text + offset, size - offset);
        if (count < 0 && errno == EINTR) continue;
        if (count <= 0) { free(text); close(fd); return NULL; }
        offset += (size_t)count;
    }
    close(fd);
    return text;
}

static int setting(PyConfig *config, wchar_t **field, const char *value) {
    PyStatus status = PyConfig_SetBytesString(config, field, value);
    return PyStatus_Exception(status) ? -1 : 0;
}

static int initialize(char **argv) {
    PyConfig config;
    PyConfig_InitIsolatedConfig(&config);
    config.site_import = 0;
    config.write_bytecode = 0;
    config.parse_argv = 0;
    int failed = setting(&config, &config.program_name, argv[1]) |
        setting(&config, &config.executable, argv[1]) |
        setting(&config, &config.base_executable, argv[1]) |
        setting(&config, &config.home, argv[2]) |
        setting(&config, &config.prefix, argv[2]) |
        setting(&config, &config.base_prefix, argv[2]) |
        setting(&config, &config.exec_prefix, argv[2]) |
        setting(&config, &config.base_exec_prefix, argv[2]);
    PyStatus status = PyConfig_SetBytesArgv(&config, 2, argv + 4);
    if (!failed && !PyStatus_Exception(status)) {
        status = Py_InitializeFromConfig(&config);
        failed = PyStatus_Exception(status);
    } else failed = 1;
    PyConfig_Clear(&config);
    if (failed) return -1;
    /* Trusted stdlib is loaded before policy. Isolated config suppresses the
     * working directory, PYTHONPATH, user site and startup-file execution. */
    return PyRun_SimpleString("import ctypes, json, os, socket, subprocess, sys, threading");
}

int main(int argc, char **argv) {
    if (argc != 6) {
        fprintf(stderr, "usage: proof PYTHON PREFIX PROFILE FIXTURE INPUT_JSON\n");
        return 2;
    }
    char *profile = NULL;
    if (strcmp(argv[3], "-") != 0 && (profile = profile_text(argv[3])) == NULL) {
        fprintf(stderr, "could not read bounded regular profile\n");
        return 2;
    }
    int limit = getdtablesize();
    for (int fd = 3; fd < limit; fd++) close(fd);
    if (initialize(argv) != 0) { free(profile); return 2; }
    if (profile != NULL) {
        char *error = NULL;
        /* This legacy API/custom-profile mechanism is deprecated in the SDK.
         * A successful probe is current-host evidence, not a support promise. */
        int result = sandbox_init(profile, 0, &error);
        free(profile);
        if (result != 0) {
            fprintf(stderr, "Seatbelt admission failed: %s\n", error ? error : "unknown");
            sandbox_free_error(error);
            return 2;
        }
        sandbox_free_error(error);
    }
    FILE *fixture = fopen(argv[4], "rb");
    if (fixture == NULL) { perror("fixture"); return 2; }
    int result = PyRun_SimpleFileEx(fixture, argv[4], 1);
    if (Py_FinalizeEx() < 0) return 2;
    return result == 0 ? 0 : 1;
}
