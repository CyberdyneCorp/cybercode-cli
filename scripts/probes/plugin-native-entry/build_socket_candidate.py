"""Build an isolated C experiment; never mutate production helper sources."""
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

root = Path(__file__).resolve().parents[3]
output = Path(sys.argv[1]).resolve()
mode = sys.argv[2] if len(sys.argv) == 3 else "seccomp"
assert mode in {"seccomp", "broker", "broker-no-bind", "broker-no-thread", "landlock9", "landlock9-no-resolve"}
unix9 = mode.startswith("landlock9")
with tempfile.TemporaryDirectory(prefix="cyber-socket-candidate-") as temporary:
    target = Path(temporary)
    for name in ["linux_plugin_exec.c", "linux_filter.h", "linux_initial_exec.h"]:
        shutil.copyfile(root / "crates/cyber-sandbox/native" / name, target / name)
    family = "linux_socket_landlock9.h" if unix9 else "linux_socket_candidate.h"
    shutil.copyfile(Path(__file__).with_name(family), target / "socket_candidate.h")
    if mode.startswith("broker"):
        shutil.copyfile(Path(__file__).with_name("linux_socket_broker.h"), target / "socket_broker.h")
        if mode == "broker-no-thread":
            broker = target / "socket_broker.h"
            text = broker.read_text()
            before = "syscall(SYS_pidfd_open, request->pid, PIDFD_THREAD)"
            assert text.count(before) == 1
            broker.write_text(text.replace(before, "syscall(SYS_pidfd_open, request->pid, 0)"))
        # Preserve family/pair restrictions, replacing the blanket IP server
        # denial with the candidate's pinned-object syscall mediation.
        socket_header = target / "socket_candidate.h"
        text = socket_header.read_text()
        for name in ["bind", "listen"]:
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
            for name in ["bind", "listen", "connect"])
        initial_header.write_text(text.replace(before, before + notify))
        source = target / "linux_plugin_exec.c"
        text = source.read_text()
        marker = "int cyber_plugin_exec(int argc, const char *const argv[]) {"
        assert text.count(marker) == 1
        prefix, entry = text.split(marker)
        boundary = "    if (!single_threaded() || confine(argv[1]) != 0) {"
        assert entry.count(boundary) == 1 and entry.endswith("}\n")
        arguments = entry.split(boundary)[0]
        source.write_text(prefix + '#include "socket_broker.h"\n\n' + marker + arguments +
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
