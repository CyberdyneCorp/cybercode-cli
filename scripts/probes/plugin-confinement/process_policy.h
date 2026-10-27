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

static inline int proof_replacement_denied(void) {
#ifdef __linux__
    /* Positive controls need permission to install a filter too. Setting
     * no_new_privs only removes privilege and cannot weaken an existing policy. */
    if (prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0) return 0;
    struct sock_filter allow = BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW);
    struct sock_fprog program = { .len = 1, .filter = &allow };
    int syscall_result = syscall(SYS_seccomp, SECCOMP_SET_MODE_FILTER, 0, &program);
    int prctl_result = prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &program, 0, 0);
    return syscall_result < 0 && prctl_result < 0;
#else
    return 0;
#endif
}

#endif
