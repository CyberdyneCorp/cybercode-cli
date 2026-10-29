/* Test-only Node-API bootstrap. The core never loads this into its own process. */
#include <node_api.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>
#include "process_policy.h"

static char *value_text(napi_env env, napi_value value, size_t maximum) {
    size_t size = 0;
    if (napi_get_value_string_utf8(env, value, NULL, 0, &size) != napi_ok || size > maximum) {
        napi_throw_type_error(env, NULL, "bounded strings are required");
        return NULL;
    }
    char *text = calloc(size + 1, 1);
    if (text == NULL) { napi_throw_error(env, NULL, "allocation failed"); return NULL; }
    size_t copied = 0;
    if (napi_get_value_string_utf8(env, value, text, size + 1, &copied) != napi_ok ||
        copied != size || strlen(text) != size) {
        free(text);
        napi_throw_type_error(env, NULL, "invalid string or embedded NUL");
        return NULL;
    }
    return text;
}

static char *argument(napi_env env, napi_callback_info info, size_t maximum) {
    size_t argc = 1;
    napi_value value;
    if (napi_get_cb_info(env, info, &argc, &value, NULL, NULL) != napi_ok || argc != 1) {
        napi_throw_type_error(env, NULL, "one bounded string is required");
        return NULL;
    }
    return value_text(env, value, maximum);
}

static napi_value boolean(napi_env env, int value) {
    napi_value result;
    if (napi_get_boolean(env, value != 0, &result) != napi_ok) return NULL;
    return result;
}

static napi_value apply(napi_env env, napi_callback_info info) {
    char *profile = argument(env, info, 65536);
    if (profile == NULL) return NULL;
    int result = proof_apply(profile);
    free(profile);
    if (result != 0) {
        napi_throw_error(env, NULL, "native execution policy admission failed");
        return NULL;
    }
    return boolean(env, 1);
}

static napi_value native_read(napi_env env, napi_callback_info info) {
    char *path = argument(env, info, 16384);
    if (path == NULL) return NULL;
    int fd = open(path, O_RDONLY);
    free(path);
    char byte;
    int success = fd >= 0 && read(fd, &byte, 1) == 1;
    if (fd >= 0) close(fd);
    return boolean(env, success);
}

static napi_value relax(napi_env env, napi_callback_info info) {
    (void)info;
    return boolean(env, proof_relax() == 0);
}

static napi_value replacement_denied(napi_env env, napi_callback_info info) {
    (void)info;
    return boolean(env, proof_replacement_denied());
}

static napi_value native_fork(napi_env env, napi_callback_info info) {
    (void)info;
    pid_t pid = fork();
    if (pid < 0) return boolean(env, 0);
    if (pid == 0) _exit(0);
    int status;
    pid_t waited;
    do { waited = waitpid(pid, &status, 0); } while (waited < 0 && errno == EINTR);
    return boolean(env, waited == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0);
}

static napi_value native_write(napi_env env, napi_callback_info info) {
    char *path = argument(env, info, 16384);
    if (path == NULL) return NULL;
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0600);
    free(path);
    int success = fd >= 0 && write(fd, "fixture only", 12) == 12;
    if (fd >= 0) close(fd);
    return boolean(env, success);
}

static napi_value self_exec(napi_env env, napi_callback_info info) {
    size_t argc = 3;
    napi_value values[3];
    if (napi_get_cb_info(env, info, &argc, values, NULL, NULL) != napi_ok || argc != 3) {
        napi_throw_type_error(env, NULL, "runtime path, receipt path and runtime kind are required");
        return NULL;
    }
    char *path = value_text(env, values[0], 16384);
    if (path == NULL) return NULL;
    char *receipt = value_text(env, values[1], 16384);
    if (receipt == NULL) { free(path); return NULL; }
    char *kind = value_text(env, values[2], 16);
    if (kind == NULL) { free(path); free(receipt); return NULL; }
    if (strcmp(kind, "deno") == 0) {
        execl(path, path, "eval", "--no-config", "--no-lock",
              "import fs from 'node:fs';fs.writeFileSync(Deno.args[0],JSON.stringify({self_exec:true}))", receipt, (char *)NULL);
    } else {
        execl(path, path, "-e", "require(\"node:fs\").writeFileSync(process.argv[1],JSON.stringify({self_exec:true}))", receipt, (char *)NULL);
    }
    free(path);
    free(receipt);
    free(kind);
    return boolean(env, 0);
}

static napi_value initialize(napi_env env, napi_value exports) {
    const napi_property_descriptor properties[] = {
        {"apply", NULL, apply, NULL, NULL, NULL, napi_default, NULL},
        {"read", NULL, native_read, NULL, NULL, NULL, napi_default, NULL},
        {"write", NULL, native_write, NULL, NULL, NULL, napi_default, NULL},
        {"selfExec", NULL, self_exec, NULL, NULL, NULL, napi_default, NULL},
        {"relax", NULL, relax, NULL, NULL, NULL, napi_default, NULL},
        {"fork", NULL, native_fork, NULL, NULL, NULL, napi_default, NULL},
        {"replacementDenied", NULL, replacement_denied, NULL, NULL, NULL, napi_default, NULL},
    };
    if (napi_define_properties(env, exports, 7, properties) != napi_ok) return NULL;
    return exports;
}

NAPI_MODULE(proof, initialize)
