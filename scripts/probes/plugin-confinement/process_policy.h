/* Experimental proof mechanisms only; never included by production code. */
#ifndef CYBER_PROOF_PROCESS_POLICY_H
#define CYBER_PROOF_PROCESS_POLICY_H

#ifdef __APPLE__
#include <sandbox.h>
static int proof_apply(const char *profile) {
    char *error = NULL;
    int result = sandbox_init(profile, 0, &error);
    sandbox_free_error(error);
    return result;
}
#elif defined(__linux__)
#include "linux_filter.h"
#else
#error "No native proof mechanism for this platform"
#endif

static inline int proof_relax(void) {
#ifdef __APPLE__
    return proof_apply("(version 1)(allow default)");
#else
    return prctl(PR_SET_SECCOMP, SECCOMP_MODE_DISABLED, 0, 0, 0);
#endif
}

#endif
