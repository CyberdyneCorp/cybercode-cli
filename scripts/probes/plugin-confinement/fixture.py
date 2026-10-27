"""Adversarial operations on driver-owned temporary fixtures only."""

import ctypes
import json
import os
import subprocess
import sys
import threading


case = json.loads(sys.argv[1])
libc = ctypes.CDLL(None, use_errno=True)
libc.open.argtypes = [ctypes.c_char_p, ctypes.c_int]
libc.open.restype = ctypes.c_int
libc.read.argtypes = [ctypes.c_int, ctypes.c_void_p, ctypes.c_size_t]
libc.read.restype = ctypes.c_ssize_t
libc.execv.argtypes = [ctypes.c_char_p, ctypes.POINTER(ctypes.c_char_p)]
libc.execv.restype = ctypes.c_int


def native_read(path):
    fd = libc.open(os.fsencode(path), os.O_RDONLY, 0)
    if fd < 0:
        return False
    try:
        buffer = ctypes.create_string_buffer(32)
        return libc.read(fd, buffer, len(buffer)) > 0
    finally:
        os.close(fd)


def native_write(path):
    fd = libc.open(os.fsencode(path), os.O_WRONLY | os.O_CREAT | os.O_TRUNC, ctypes.c_int(0o600))
    if fd < 0:
        return False
    try:
        return os.write(fd, b"fixture only") == 12
    finally:
        os.close(fd)


def can_list(path):
    try:
        os.listdir(path)
        return True
    except OSError:
        return False


def cannot_relax():
    if not case["confined"]:
        return None
    if case["platform"] == "linux":
        # Linux forbids disabling installed seccomp filters. This direct libc
        # call tests the native primitive rather than a language API guard.
        result = libc.prctl(22, 0, 0, 0, 0)  # PR_SET_SECCOMP, SECCOMP_MODE_DISABLED
        return result != 0 and not native_read(case["private_file"])
    apply = libc.sandbox_init
    apply.argtypes = [ctypes.c_char_p, ctypes.c_uint64, ctypes.POINTER(ctypes.c_void_p)]
    apply.restype = ctypes.c_int
    release = libc.sandbox_free_error
    release.argtypes = [ctypes.c_void_p]
    error = ctypes.c_void_p()
    result = apply(b"(version 1)(allow default)", 0, ctypes.byref(error))
    if error.value:
        release(error)
    return result != 0 and not native_read(case["private_file"])


def hardlink_private():
    alias = os.path.join(case["scratch"], "private-alias")
    try:
        os.link(case["private_file"], alias)
        return native_read(alias)
    except OSError:
        return False


def rename_project():
    target = os.path.join(case["scratch"], "moved-project")
    try:
        os.rename(case["project"], target)
    except OSError:
        return False
    os.rename(target, case["project"])
    return True


def spawn_shell():
    try:
        pid = os.posix_spawn("/bin/sh", ["/bin/sh", "-c", "exit 0"], {})
        return os.waitpid(pid, 0)[1] == 0
    except OSError:
        return False


def child_reads():
    script = """
import ctypes, json, os, sys
libc = ctypes.CDLL(None)
def check(path):
    fd = libc.open(os.fsencode(path), os.O_RDONLY)
    if fd < 0: return False
    os.close(fd)
    return True
print(json.dumps([check(sys.argv[1]), check(sys.argv[2])]))
"""
    try:
        result = subprocess.run(
            [case["python"], "-I", "-S", "-c", script, case["project_file"], case["private_file"]],
            check=False, capture_output=True, text=True, timeout=5,
            env={"PATH": "/usr/bin:/bin"},
        )
    except OSError:
        return None
    if result.returncode != 0:
        raise RuntimeError("declared child could not initialize (" + str(result.returncode) + "), executable_read=" + str(native_read(case["python"])) + ", executable=" + os.path.realpath(case["python"]) + ": " + result.stderr)
    return json.loads(result.stdout)


def self_exec():
    # libc execv is direct native execution, not a subprocess API interception.
    program = os.fsencode(case["python"])
    script = b'import json; print(json.dumps({"self_exec": True}))'
    arguments = (ctypes.c_char_p * 6)(program, b"-I", b"-S", b"-c", script, None)
    result = libc.execv(program, arguments)
    if result != -1:
        raise RuntimeError("execv returned an unexpected result")
    print(json.dumps({"self_exec": False}))


def native_child_reads():
    results = []
    for path in [case["project_file"], case["private_file"]]:
        try:
            result = subprocess.run(["/bin/cat", path], capture_output=True, timeout=5, env={})
        except OSError:
            return None
        results.append(result.returncode == 0)
    return results


def native_fork():
    # Child does no runtime work after fork; leave directly through libc.
    pid = libc.fork()
    if pid < 0:
        return False
    if pid == 0:
        libc._exit(0)
    return os.waitpid(pid, 0)[1] == 0


def filter_replacement_denied():
    if libc.prctl(38, 1, 0, 0, 0) != 0:  # PR_SET_NO_NEW_PRIVS
        raise RuntimeError("could not prepare seccomp replacement control")
    class Filter(ctypes.Structure):
        _fields_ = [("code", ctypes.c_ushort), ("jt", ctypes.c_ubyte),
                    ("jf", ctypes.c_ubyte), ("value", ctypes.c_uint32)]
    class Program(ctypes.Structure):
        _fields_ = [("length", ctypes.c_ushort), ("filter", ctypes.POINTER(Filter))]
    instruction = Filter(6, 0, 0, 0x7fff0000)  # BPF_RET|BPF_K, SECCOMP_RET_ALLOW
    program = Program(1, ctypes.pointer(instruction))
    return libc.prctl(22, 2, ctypes.byref(program), 0, 0) < 0


def inspect():
    thread_result = []
    worker = threading.Thread(target=lambda: thread_result.append([
        [native_read(case["project_file"]), native_read(case["private_file"])], spawn_shell()]))
    worker.start()
    worker.join(timeout=2)
    inherited = False
    try:
        identity = os.fstat(9)
        inherited = (str(identity.st_dev) == case["private_dev"] and
                     str(identity.st_ino) == case["private_ino"])
    except OSError:
        pass
    observations = {
        "package_read": native_read(case["package_file"]),
        "scratch_write": native_write(case["scratch_file"]),
        "scratch_read": native_read(case["scratch_file"]),
        "project_read": native_read(case["project_file"]),
        "project_list": can_list(os.path.dirname(case["project_file"])),
        "private_list": can_list(os.path.dirname(case["private_file"])),
        "private_write": native_write(case["private_file"]),
        "protected_read": native_read(case["protected_file"]),
        "protected_write": native_write(case["protected_file"]),
        "hardlink_private": hardlink_private(),
        "cannot_relax": cannot_relax(),
        "private_read": native_read(case["private_file"]),
        "symlink_private_read": native_read(case["symlink_private"]),
        "project_write": native_write(case["project_write"]),
        "inherited_descriptor": inherited,
        "thread": len(thread_result) == 1 and not worker.is_alive(),
        "thread_reads": thread_result[0][0] if thread_result else None,
        "thread_shell": thread_result[0][1] if thread_result else None,
        "shell": spawn_shell(),
        "native_fork": native_fork(),
        "child_reads": child_reads(),
        "native_child_reads": native_child_reads(),
    }
    observations["written_project_read"] = native_read(case["project_write"])
    observations["project_rename"] = rename_project()
    if case["initial_exec"]:
        observations["filter_replacement_denied"] = filter_replacement_denied()
    print(json.dumps(observations, sort_keys=True))


if case["mode"] == "exec":
    self_exec()
else:
    inspect()
