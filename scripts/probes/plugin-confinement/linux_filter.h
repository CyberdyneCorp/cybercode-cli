/* Test-only execution filter. Filesystem isolation is installed before runtime
 * startup by linux_exec.c; the outer mount namespace masks protected subtrees.
 * TSYNC is mandatory: Node already owns runtime threads at this point. */
#ifndef CYBER_PROOF_LINUX_FILTER_H
#define CYBER_PROOF_LINUX_FILTER_H
#include <errno.h>
#include <linux/audit.h>
#include <linux/filter.h>
#include <linux/sched.h>
#include <linux/seccomp.h>
#include <stddef.h>
#include <string.h>
#include <sys/prctl.h>
#include <sys/syscall.h>
#include <unistd.h>

#if defined(__x86_64__)
#define PROOF_ARCH AUDIT_ARCH_X86_64
#elif defined(__aarch64__) && __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__
#define PROOF_ARCH AUDIT_ARCH_AARCH64
#else
#error "Linux proof requires native x86_64 or little-endian aarch64"
#endif

#define PROOF_DENY(nr) \
    BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, (nr), 0, 1), \
    BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM)
#define PROOF_ARCH_CHECK \
    BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, arch)), \
    BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, PROOF_ARCH, 1, 0), \
    BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS), \
    BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, nr)), \
    BPF_JUMP(BPF_JMP | BPF_JGE | BPF_K, 0x40000000U, 0, 1), \
    BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS)

#define PROOF_ESCAPE_DENIALS \
    PROOF_DENY(SYS_ptrace), \
    PROOF_DENY(SYS_process_vm_readv), \
    PROOF_DENY(SYS_process_vm_writev), \
    PROOF_DENY(SYS_pidfd_getfd), \
    PROOF_DENY(SYS_open_by_handle_at), \
    PROOF_DENY(SYS_bpf), \
    PROOF_DENY(SYS_userfaultfd), \
    PROOF_DENY(SYS_io_uring_setup), \
    PROOF_DENY(SYS_mount), \
    PROOF_DENY(SYS_umount2), \
    PROOF_DENY(SYS_pivot_root), \
    PROOF_DENY(SYS_move_mount), \
    PROOF_DENY(SYS_open_tree), \
    PROOF_DENY(SYS_fsopen), \
    PROOF_DENY(SYS_fsmount), \
    PROOF_DENY(SYS_setns), \
    PROOF_DENY(SYS_unshare)

static inline int proof_filter(struct sock_filter *instructions, size_t count) {
    struct sock_fprog program = { .len = (unsigned short)count, .filter = instructions };
    /* Positive returns identify an unsynchronized thread and are failures too. */
    return syscall(SYS_seccomp, SECCOMP_SET_MODE_FILTER,
                   SECCOMP_FILTER_FLAG_TSYNC, &program) == 0 ? 0 : -1;
}

static inline int proof_apply(const char *profile) {
    int shell;
    if (strcmp(profile, "shell=0\n") == 0) shell = 0;
    else if (strcmp(profile, "shell=1\n") == 0) shell = 1;
    else { errno = EINVAL; return -1; }
    if (prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0) return -1;
    struct sock_filter boundary[] = {
        PROOF_ARCH_CHECK,
        PROOF_ESCAPE_DENIALS,
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    if (proof_filter(boundary, sizeof(boundary) / sizeof(boundary[0])) != 0) return -1;
    if (shell) return 0;
    struct sock_filter execution[] = {
        PROOF_ARCH_CHECK,
        PROOF_DENY(SYS_execve),
        PROOF_DENY(SYS_execveat),
#ifdef SYS_fork
        PROOF_DENY(SYS_fork),
#endif
#ifdef SYS_vfork
        PROOF_DENY(SYS_vfork),
#endif
        /* libc can fall back to clone for threads; seccomp cannot inspect the
         * pointed-to clone3 flags safely, so no clone3 request is admitted. */
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_clone3, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | ENOSYS),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_clone, 0, 4),
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, args[0])),
        BPF_STMT(BPF_ALU | BPF_AND | BPF_K, CLONE_THREAD | CLONE_VM | CLONE_SIGHAND),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, CLONE_THREAD | CLONE_VM | CLONE_SIGHAND, 1, 0),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    return proof_filter(execution, sizeof(execution) / sizeof(execution[0]));
}
#endif
