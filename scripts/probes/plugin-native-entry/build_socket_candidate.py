"""Build an isolated C experiment; never mutate production helper sources."""
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

root = Path(__file__).resolve().parents[3]
output = Path(sys.argv[1]).resolve()
with tempfile.TemporaryDirectory(prefix="cyber-socket-candidate-") as temporary:
    target = Path(temporary)
    for name in ["linux_plugin_exec.c", "linux_filter.h", "linux_initial_exec.h"]:
        shutil.copyfile(root / "crates/cyber-sandbox/native" / name, target / name)
    shutil.copyfile(Path(__file__).with_name("linux_socket_candidate.h"), target / "socket_candidate.h")
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
