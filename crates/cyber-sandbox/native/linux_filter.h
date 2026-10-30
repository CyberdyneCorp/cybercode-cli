/* Native startup seccomp definitions, promoted from the confinement proof. */
#ifndef CYBER_PLUGIN_LINUX_FILTER_H
#define CYBER_PLUGIN_LINUX_FILTER_H
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
#define CYBER_ARCH AUDIT_ARCH_X86_64
#elif defined(__aarch64__) && __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__
#define CYBER_ARCH AUDIT_ARCH_AARCH64
#else
#error "Linux plugin entry requires native x86_64 or little-endian aarch64"
#endif

#define CYBER_DENY(nr) \
    BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, (nr), 0, 1), \
    BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM)
#define CYBER_ARCH_CHECK \
    BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, arch)), \
    BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, CYBER_ARCH, 1, 0), \
    BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS), \
    BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, nr)), \
    BPF_JUMP(BPF_JMP | BPF_JGE | BPF_K, 0x40000000U, 0, 1), \
    BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS)

#define CYBER_ESCAPE_DENIALS \
    CYBER_DENY(SYS_ptrace), \
    CYBER_DENY(SYS_process_vm_readv), \
    CYBER_DENY(SYS_process_vm_writev), \
    CYBER_DENY(SYS_pidfd_getfd), \
    CYBER_DENY(SYS_open_by_handle_at), \
    CYBER_DENY(SYS_bpf), \
    CYBER_DENY(SYS_userfaultfd), \
    CYBER_DENY(SYS_io_uring_setup), \
    CYBER_DENY(SYS_mount), \
    CYBER_DENY(SYS_umount2), \
    CYBER_DENY(SYS_pivot_root), \
    CYBER_DENY(SYS_move_mount), \
    CYBER_DENY(SYS_open_tree), \
    CYBER_DENY(SYS_fsopen), \
    CYBER_DENY(SYS_fsmount), \
    CYBER_DENY(SYS_setns), \
    CYBER_DENY(SYS_unshare)

#endif
