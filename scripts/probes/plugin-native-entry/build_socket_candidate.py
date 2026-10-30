"""Build an isolated C experiment; never mutate production helper sources."""
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

root = Path(__file__).resolve().parents[3]
output = Path(sys.argv[1]).resolve()
mode = sys.argv[2] if len(sys.argv) == 3 else "seccomp"
assert mode in {"seccomp", "broker", "broker-no-bind", "broker-no-thread", "broker-audit", "broker-audit-no-cancel", "broker-audit-wrong-cap",
                "broker-mutation", "broker-mutation-no-pin", "broker-mutation-no-copy", "broker-mutation-continue",
                "broker-private-stream", "broker-private-stream-no-makesock", "broker-private-stream-no-refer",
                "broker-private-stream-mutation", "broker-private-stream-mutation-no-endpoint",
                "broker-private-stream-wrong-cwd", "broker-private-stream-no-beneath",
                "landlock9", "landlock9-no-resolve"}
unix9 = mode.startswith("landlock9")
private_stream = mode.startswith("broker-private-stream")
unix_mutation = mode.startswith("broker-private-stream-mutation")
with tempfile.TemporaryDirectory(prefix="cyber-socket-candidate-") as temporary:
    target = Path(temporary)
    for name in ["linux_plugin_exec.c", "linux_filter.h", "linux_initial_exec.h"]:
        shutil.copyfile(root / "crates/cyber-sandbox/native" / name, target / name)
    family = "linux_socket_landlock9.h" if unix9 else (
        "linux_socket_private_stream.h" if private_stream else "linux_socket_candidate.h")
    shutil.copyfile(Path(__file__).with_name(family), target / "socket_candidate.h")
    if mode.startswith("broker"):
        shutil.copyfile(Path(__file__).with_name("linux_socket_broker.h"), target / "socket_broker.h")
        if private_stream:
            shutil.copyfile(Path(__file__).with_name("linux_socket_private_unix.h"), target / "socket_private_unix.h")
            if mode == "broker-private-stream-no-makesock":
                header = target / "socket_private_unix.h"
                text = header.read_text()
                before = "    if (result == 0) result = syscall(SYS_landlock_restrict_self, ruleset, 0);"
                assert text.count(before) == 1
                header.write_text(text.replace(before, "    /* Missing-rule control: do not install scratch-only MAKE_SOCK. */"))
            if mode == "broker-private-stream-no-refer":
                header = target / "socket_private_unix.h"
                text = header.read_text()
                before = '    if (result == 0) result = add_path(ruleset, "/", LANDLOCK_ACCESS_FS_REFER);'
                assert text.count(before) == 1
                header.write_text(text.replace(before, "    /* Missing compatibility grant: REFER remains denied. */"))
            if mode == "broker-private-stream-wrong-cwd":
                header = target / "socket_private_unix.h"
                text = header.read_text()
                before = '    snprintf(proc, sizeof(proc), "/proc/%u/cwd", tid);'
                assert text.count(before) == 1
                header.write_text(text.replace(before, '    tid = (unsigned int)getpid(); /* Wrong-context control. */\n' + before))
            if mode == "broker-private-stream-no-beneath":
                header = target / "socket_private_unix.h"
                text = header.read_text()
                before = '''    struct open_how how = { .flags = O_PATH | O_CLOEXEC,
        .resolve = RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS | RESOLVE_NO_SYMLINKS };'''
                assert text.count(before) == 1
                header.write_text(text.replace(before, before.replace('RESOLVE_BENEATH | ', '')))
        if mode == "broker-audit-wrong-cap":
            broker = target / "socket_broker.h"
            text = broker.read_text()
            before = "#define BROKER_SLOTS 16"
            assert text.count(before) == 1
            broker.write_text(text.replace(before, "#define BROKER_SLOTS 17"))
        if mode == "broker-no-thread":
            broker = target / "socket_broker.h"
            text = broker.read_text()
            before = "syscall(SYS_pidfd_open, request->pid, PIDFD_THREAD)"
            assert text.count(before) == 1
            broker.write_text(text.replace(before, "syscall(SYS_pidfd_open, request->pid, 0)"))
        if mode.startswith("broker-audit") or mode.startswith("broker-mutation") or unix_mutation:
            # Fixture-only receipts from the actual worker owner. Logging is
            # injected solely into this temporary copy, never the base helper.
            broker = target / "socket_broker.h"
            text = broker.read_text()
            before = "    jobs[slot] = (struct broker_job){ .pid = worker, .pipe = channel[0], .id = request->id };\n"
            assert text.count(before) == 1
            text = text.replace(before, before + '    fprintf(stderr, "BSTART %d\\n", (int)worker);\n')
            before = "            (void)kill(job->pid, SIGKILL);\n"
            assert text.count(before) == 1
            cancel = '' if mode == "broker-audit-no-cancel" else before
            text = text.replace(before, '            fprintf(stderr, "BCANCEL %d\\n", (int)job->pid);\n' + cancel)
            before = "        *job = (struct broker_job){0};\n"
            assert text.count(before) == 1
            text = text.replace(before, '        fprintf(stderr, "BREAP %d\\n", (int)job->pid);\n' + before)
            broker.write_text(text)
        if mode.startswith("broker-mutation") or unix_mutation:
            shutil.copyfile(Path(__file__).with_name("linux_socket_broker_mutation.h"), target / "socket_mutation.h")
            if unix_mutation:
                hook = target / "socket_mutation.h"
                text = hook.read_text()
                before = '    if (request->data.nr != SYS_bind || request->data.args[2] != sizeof(struct sockaddr_storage)) return 0;'
                assert text.count(before) == 1
                hook.write_text(text.replace(before, '    if (request->data.nr != SYS_connect || request->data.args[2] != sizeof(struct sockaddr_un)) return 0;'))
            broker = target / "socket_broker.h"
            text = broker.read_text()
            before = "static void broker_tick(int signal_number)"
            assert text.count(before) == 1
            text = text.replace(before, '#include "socket_mutation.h"\n\n' + before)
            before = "    call->socket = socket;\n    return 0;"
            assert text.count(before) == 1
            mutation = '''    int mutated = broker_mutation_pause(request);
    if (mutated < 0) {
        if (call->endpoint >= 0) close(call->endpoint);
        close(socket); return EIO;
    }
'''
            if mode == "broker-private-stream-mutation-no-endpoint":
                mutation += '''    if (mutated && call->endpoint >= 0) {
        close(call->endpoint); call->endpoint = -1;
    }
'''
            if mode == "broker-mutation-no-pin":
                mutation += '''    if (mutated) {
        int thread = syscall(SYS_pidfd_open, request->pid, PIDFD_THREAD);
        int replaced = thread < 0 ? -1 : syscall(SYS_pidfd_getfd, thread, (int)request->data.args[0], 0);
        if (thread >= 0) close(thread);
        close(socket);
        if (replaced < 0) return EIO;
        socket = replaced;
    }
'''
            if mode == "broker-mutation-no-copy":
                mutation += '''    if (mutated) {
        struct iovec local = { .iov_base = &call->address, .iov_len = call->length };
        struct iovec remote = { .iov_base = (void *)(uintptr_t)request->data.args[1], .iov_len = call->length };
        if (process_vm_readv(request->pid, &local, 1, &remote, 1, 0) != (ssize_t)call->length) {
            close(socket); return EIO;
        }
    }
'''
            text = text.replace(before, mutation + before)
            if mode == "broker-mutation-continue":
                before = "    struct seccomp_notif_resp response = { .id = id, .val = value, .error = -error };\n"
                assert text.count(before) == 1
                text = text.replace(before, before + '    if (broker_mutation_seen && id == broker_mutation_id) response.flags = SECCOMP_USER_NOTIF_FLAG_CONTINUE;\n')
            broker.write_text(text)
        # Preserve family/pair restrictions, replacing the blanket IP server
        # denial with the candidate's pinned-object syscall mediation.
        socket_header = target / "socket_candidate.h"
        text = socket_header.read_text()
        for name in ["bind", "listen"]:
            if private_stream:
                continue
            if mode == "broker-no-bind" and name == "bind":
                continue
            line = f"    CYBER_DENY(SYS_{name}), \\\n"
            assert text.count(line) == 1
            text = text.replace(line, "")
        socket_header.write_text(text)
        initial_header = target / "linux_initial_exec.h"
        text = initial_header.read_text()
        # Linux permits only one listener in this inherited filter tree. The
        # candidate's parent owns the one-use startup budget and socket service.
        before = "static int continue_initial_exec(int listener, pid_t expected) {"
        assert text.count(before) == 1
        text = text.split(before)[0] + "#endif\n"
        before = "shell ? 0 : SECCOMP_FILTER_FLAG_NEW_LISTENER, &program"
        assert text.count(before) == 1
        text = text.replace(before, "SECCOMP_FILTER_FLAG_NEW_LISTENER, &program")
        before = "        CYBER_ESCAPE_DENIALS,\n"
        assert text.count(before) == 1
        notify = "".join(
            f"        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_{name}, 0, 1),\n"
            "        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_USER_NOTIF),\n"
            for name in (["connect"] if private_stream else ["bind", "listen", "connect"]))
        initial_header.write_text(text.replace(before, before + notify))
        source = target / "linux_plugin_exec.c"
        text = source.read_text()
        if private_stream:
            before = "        if ((line[0] != 'R' && line[0] != 'W')"
            assert text.count(before) == 1
            text = text.replace(before, "        if (line[0] == 'U') continue;\n" + before)
        marker = "int cyber_plugin_exec(int argc, const char *const argv[]) {"
        assert text.count(marker) == 1
        prefix, entry = text.split(marker)
        boundary = "    if (!single_threaded() || confine(argv[1]) != 0) {"
        assert entry.count(boundary) == 1 and entry.endswith("}\n")
        arguments = entry.split(boundary)[0]
        private_define = '#define CYBER_PRIVATE_UNIX_STREAM 1\n' if private_stream else ''
        source.write_text(prefix + private_define + '#include "socket_broker.h"\n\n' + marker + arguments +
            '    if (!single_threaded() || syscall(SYS_close_range, 3U, UINT_MAX, 0) != 0) return 2;\n'
            '    return broker_entry(argv, shell);\n}\n')
    if unix9:
        shutil.copyfile(Path(__file__).with_name("linux_unix9_policy.h"), target / "unix9_policy.h")
        if mode == "landlock9-no-resolve":
            policy = target / "unix9_policy.h"
            text = policy.read_text()
            before = "#define CYBER_RESOLVE_UNIX (1ULL << 16)"
            assert text.count(before) == 1
            policy.write_text(text.replace(before, "#define CYBER_RESOLVE_UNIX 0ULL"))
        source = target / "linux_plugin_exec.c"
        text = source.read_text()
        before = '        if ((line[0] != \'R\' && line[0] != \'W\')'
        assert text.count(before) == 1
        text = text.replace(before, '        if (line[0] == \'U\') continue;\n' + before)
        before = 'if (!single_threaded() || confine(argv[1]) != 0)'
        assert text.count(before) == 1
        text = text.replace(before, 'if (!single_threaded() || confine_unix9(argv[1]) != 0 || confine(argv[1]) != 0)')
        before = 'int cyber_plugin_exec(int argc, const char *const argv[]) {'
        assert text.count(before) == 1
        source.write_text(text.replace(before, '#include "unix9_policy.h"\n\n' + before))
    header = target / "linux_filter.h"
    text = header.read_text()
    before = "#define CYBER_ESCAPE_DENIALS \\\n"
    assert text.count(before) == 1
    header.write_text(text.replace(before, '#include "socket_candidate.h"\n' + before + "    CYBER_SOCKET_FAMILIES, \\\n"))
    wrapper = target / "main.c"
    wrapper.write_text('''#include <string.h>
int cyber_plugin_exec(int, const char *const []);
int main(int argc, char **argv) {
    if (argc < 2 || strcmp(argv[1], "--plugin-native-exec") != 0) return 2;
    return cyber_plugin_exec(argc - 1, (const char *const *)(argv + 1));
}
''')
    subprocess.run(["cc", "-Wall", "-Wextra", "-Werror", str(wrapper),
                    str(target / "linux_plugin_exec.c"), "-o", str(output)],
                   check=True, timeout=30)
