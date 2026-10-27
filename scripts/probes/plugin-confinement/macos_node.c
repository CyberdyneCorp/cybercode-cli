/* Test-only Node-API bootstrap. The core never loads this into its own process. */
#include <node_api.h>
#include <fcntl.h>
#include <sandbox.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static char *argument(napi_env env, napi_callback_info info, size_t maximum) {
    size_t argc = 1, size = 0;
    napi_value value;
    if (napi_get_cb_info(env, info, &argc, &value, NULL, NULL) != napi_ok || argc != 1 ||
        napi_get_value_string_utf8(env, value, NULL, 0, &size) != napi_ok || size > maximum) {
        napi_throw_type_error(env, NULL, "one bounded string is required");
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

static napi_value boolean(napi_env env, int value) {
    napi_value result;
    if (napi_get_boolean(env, value != 0, &result) != napi_ok) return NULL;
    return result;
}

static napi_value apply(napi_env env, napi_callback_info info) {
    char *profile = argument(env, info, 65536);
    if (profile == NULL) return NULL;
    char *error = NULL;
    int result = sandbox_init(profile, 0, &error);
    free(profile);
    sandbox_free_error(error);
    if (result != 0) {
        napi_throw_error(env, NULL, "Seatbelt admission failed");
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
    char *path = argument(env, info, 16384);
    if (path == NULL) return NULL;
    execl(path, path, "-e", "require(\"node:fs\").writeFileSync(process.env.CYBER_PROOF_MARKER,JSON.stringify({self_exec:true}))", (char *)NULL);
    free(path);
    return boolean(env, 0);
}

static napi_value initialize(napi_env env, napi_value exports) {
    const napi_property_descriptor properties[] = {
        {"apply", NULL, apply, NULL, NULL, NULL, napi_default, NULL},
        {"read", NULL, native_read, NULL, NULL, NULL, napi_default, NULL},
        {"write", NULL, native_write, NULL, NULL, NULL, napi_default, NULL},
        {"selfExec", NULL, self_exec, NULL, NULL, NULL, napi_default, NULL},
    };
    if (napi_define_properties(env, exports, 4, properties) != napi_ok) return NULL;
    return exports;
}

NAPI_MODULE(proof, initialize)
