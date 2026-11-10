#!/usr/bin/env python3
"""Runs a build tool confined, so code Kiwi did not write cannot reach Kiwi's own files.

On Linux a tool runs through this file as a small wrapper that confines itself and then
replaces itself with the tool (execve), so the confinement covers the tool and everything
it starts. Nothing here needs root, a setuid helper or extra container privileges:

- Landlock (Linux 6.2 or later, ABI 3+): the tool reads only the paths it was given, writes
  only beneath its working folders, and (ABI 4+) may not bind or connect TCP sockets; with
  ABI 6+ it can neither signal processes outside the sandbox nor reach abstract Unix
  sockets. Kiwi's data directory, its signing key, other jobs and artifacts are simply not
  in the allowed set.
- A seccomp filter refuses socket() altogether, so no network of any kind (TCP, UDP, Unix
  sockets such as tailscaled's or Docker's, netlink), plus io_uring, bpf, perf events,
  userfaultfd and the kernel keyrings.
- no_new_privs, and resource limits: no core dumps, a bounded file size and CPU time.

The caller decides how strict to be (`mode`): "required" refuses to run the tool when the
computer cannot confine it; "best-effort" runs it with whatever confinement is available
and records in the private build log what was missing. macOS has no sandbox Kiwi can use
here, so "required" fails there and "best-effort" runs the tool as it is.

Standard library only; `python3 sandbox.py --probe` prints what this computer supports.
"""
import ctypes
import errno
import json
import os
from pathlib import Path
import platform
import resource
import signal
import stat
import struct
import subprocess
import sys

# sys.platform, kept here so that tests can stand in for another system.
PLATFORM = sys.platform
REQUIRED, BEST_EFFORT = "required", "best-effort"
MODES = (REQUIRED, BEST_EFFORT)
# The oldest Landlock that also controls truncate(2); without it a tool could empty any file
# it can name, such as the signing key.
MIN_ABI = 3

# --- Landlock (include/uapi/linux/landlock.h) ---------------------------------------------
SYS_CREATE_RULESET, SYS_ADD_RULE, SYS_RESTRICT_SELF = 444, 445, 446  # the same on every architecture
CREATE_RULESET_VERSION = 1
RULE_PATH_BENEATH = 1
EXECUTE, WRITE_FILE, READ_FILE, READ_DIR = 1 << 0, 1 << 1, 1 << 2, 1 << 3
REMOVE_DIR, REMOVE_FILE, MAKE_CHAR, MAKE_DIR = 1 << 4, 1 << 5, 1 << 6, 1 << 7
MAKE_REG, MAKE_SOCK, MAKE_FIFO, MAKE_BLOCK, MAKE_SYM = 1 << 8, 1 << 9, 1 << 10, 1 << 11, 1 << 12
REFER, TRUNCATE, IOCTL_DEV = 1 << 13, 1 << 14, 1 << 15
FILE_RIGHTS = EXECUTE | WRITE_FILE | READ_FILE | TRUNCATE | IOCTL_DEV
NET_BIND_TCP, NET_CONNECT_TCP = 1 << 0, 1 << 1
SCOPE_ABSTRACT_UNIX_SOCKET, SCOPE_SIGNAL = 1 << 0, 1 << 1
PR_SET_NO_NEW_PRIVS, PR_SET_CHILD_SUBREAPER = 38, 36


def handled_fs(abi):
    rights = (1 << 13) - 1
    if abi >= 2:
        rights |= REFER
    if abi >= 3:
        rights |= TRUNCATE
    if abi >= 5:
        rights |= IOCTL_DEV
    return rights


READ = READ_FILE | READ_DIR
RUN = READ | EXECUTE
# Everything inside a working folder, except device nodes (which need privileges anyway).
WRITE = ((1 << 16) - 1) & ~(MAKE_CHAR | MAKE_BLOCK | IOCTL_DEV)
DEVICE = READ_FILE | WRITE_FILE

# --- seccomp ------------------------------------------------------------------------------
PR_SET_SECCOMP, SECCOMP_MODE_FILTER = 22, 2
RET_ALLOW, RET_ERRNO, RET_KILL_PROCESS = 0x7FFF0000, 0x00050000, 0x80000000
LD_ABS_W, JEQ_K, JGE_K, RET_K = 0x20, 0x15, 0x35, 0x06
# Per architecture: the audit value, then the system calls refused and the error they give.
SECCOMP = {
    "x86_64": (0xC000003E, {41: errno.EACCES,  # socket
                            425: errno.ENOSYS, 426: errno.ENOSYS, 427: errno.ENOSYS,  # io_uring
                            321: errno.EPERM,  # bpf
                            298: errno.EPERM,  # perf_event_open
                            323: errno.EPERM,  # userfaultfd
                            248: errno.EPERM, 249: errno.EPERM, 250: errno.EPERM}),  # add_key, request_key, keyctl
    "aarch64": (0xC00000B7, {198: errno.EACCES,
                             425: errno.ENOSYS, 426: errno.ENOSYS, 427: errno.ENOSYS,
                             280: errno.EPERM, 241: errno.EPERM, 282: errno.EPERM,
                             217: errno.EPERM, 218: errno.EPERM, 219: errno.EPERM}),
}
# The x32 ABI on x86-64 numbers its calls from here; the filter refuses them.
X32 = 0x40000000

# What any confined program may read: the C library, the dynamic loader's configuration,
# the user database, time zones, randomness and a little of /proc and /sys about itself
# and the machine. Missing paths are skipped.
SYSTEM_RUN = ("/usr/lib", "/usr/lib64", "/lib", "/lib64")
SYSTEM_READ = ("/etc/ld.so.cache", "/etc/ld.so.conf", "/etc/ld.so.conf.d", "/etc/passwd", "/etc/group",
               "/etc/nsswitch.conf", "/etc/localtime", "/etc/timezone", "/usr/share/zoneinfo",
               "/dev/urandom", "/dev/random", "/proc/self", "/proc/meminfo", "/proc/cpuinfo", "/proc/stat",
               "/proc/sys/vm", "/proc/sys/kernel/osrelease", "/sys/fs/cgroup", "/sys/devices/system/cpu",
               "/sys/kernel/mm/transparent_hugepage")
SYSTEM_DEVICES = ("/dev/null", "/dev/zero")

# Limits a confined tool runs with; it can lower them, never raise them.
LIMITS = {"core": 0, "fsize": 8 * 1024**3, "cpu": 3 * 3600}
RLIMITS = {"core": resource.RLIMIT_CORE, "fsize": resource.RLIMIT_FSIZE, "cpu": resource.RLIMIT_CPU}


class SandboxError(Exception):
    """The tool cannot run confined as its caller requires. The message is for the owner."""


# --- the parent's side --------------------------------------------------------------------

def environment(home, tmp, **extra):
    """A minimal environment for a build tool: nothing of Kiwi's own environment (tokens,
    proxies, sockets, paths) reaches it."""
    utf8 = "en_US.UTF-8" if PLATFORM == "darwin" else "C.UTF-8"
    return {"PATH": "/usr/bin:/bin", "LANG": utf8, "LC_ALL": utf8, "HOME": str(home), "TMPDIR": str(tmp),
            **{key: str(value) for key, value in extra.items()}}


_probed = None


def probe():
    """{"landlock": ABI or None, "seccomp": bool, "reasons": [...]} for this computer, found
    by confining a throwaway process (so the answer covers this kernel, its configuration
    and any container's seccomp profile). Cached for the life of the runner."""
    global _probed
    if _probed is not None:
        return _probed
    if PLATFORM != "linux":
        _probed = {"landlock": None, "seccomp": False, "reasons": ["this computer is not running Linux"]}
        return _probed
    try:
        done = subprocess.run([sys.executable or "python3", "-I", "-S", str(Path(__file__).resolve()), "--probe"],
                              env={"PATH": "/usr/bin:/bin"}, stdin=subprocess.DEVNULL, capture_output=True,
                              timeout=60, check=False)
        found = json.loads(done.stdout)
        result = {"landlock": found.get("landlock"), "seccomp": found.get("seccomp") is True,
                  "reasons": [str(r) for r in found.get("reasons", [])][:4]}
        if result["landlock"] is not None and type(result["landlock"]) is not int:
            raise ValueError(result)
    except (OSError, ValueError, subprocess.SubprocessError, AttributeError) as error:
        result = {"landlock": None, "seccomp": False, "reasons": [f"the sandbox check failed ({type(error).__name__})"]}
    _probed = result
    return result


def command(argv, *, mode, read=(), run=(), write=(), devices=(), limits=None, log=None):
    """The command line that runs `argv` confined to the given paths.

    `read` paths are readable, `run` paths readable and executable, `write` folders fully
    usable; everything else is out of reach. Raises SandboxError when `mode` is "required"
    and this computer cannot confine the tool; otherwise records what is missing in `log`.
    """
    if mode not in MODES:
        raise ValueError(mode)
    found = probe()
    landlock, seccomp = found["landlock"], found["seccomp"]
    missing = list(found["reasons"])
    if mode == REQUIRED and (landlock is None or not seccomp):
        raise SandboxError("Kiwi builds this app only inside a sandbox, and this computer can't provide one ("
                           + "; ".join(missing or ["unknown reason"]) + "). It needs Kiwi on Linux 6.2 or "
                           "later, or Kiwi's Docker image.")
    def note(text):
        if log is not None:
            log.write(f"kiwi: {text}\n".encode())
    if PLATFORM != "linux":
        note("sandbox unavailable: " + "; ".join(missing) + "; the tool runs unconfined")
        return [str(a) for a in argv]
    if missing:
        note("sandbox unavailable: " + "; ".join(missing))
    spec = {"landlock": landlock is not None, "seccomp": seccomp,
            "read": [str(p) for p in (*SYSTEM_READ, *read)], "run": [str(p) for p in (*SYSTEM_RUN, *run)],
            "write": [str(p) for p in write], "devices": [str(p) for p in (*SYSTEM_DEVICES, *devices)],
            "limits": dict(LIMITS, **(limits or {}))}
    if landlock is not None or seccomp:
        parts = []
        if landlock is not None:
            parts.append(f"Landlock ABI {landlock} (writes only to {', '.join(spec['write']) or 'nothing'}"
                         + ("; no TCP" if landlock >= 4 else "") + ")")
        if seccomp:
            parts.append("seccomp (no sockets)")
        note("sandbox: " + ", ".join(parts))
    return [sys.executable or "python3", "-I", "-S", str(Path(__file__).resolve()), "--spec", json.dumps(spec),
            "--", *(str(a) for a in argv)]


def adopt_orphans():
    """Makes this process the reaper of its descendants (Linux), so a confined tool that
    detaches a process (a new session escapes the job's process group) still comes back
    here, where kill_strays() ends it."""
    if PLATFORM != "linux":
        return False
    try:
        _call(_libc().prctl, ctypes.c_int(PR_SET_CHILD_SUBREAPER), ctypes.c_ulong(1), ctypes.c_ulong(0),
              ctypes.c_ulong(0), ctypes.c_ulong(0))
        return True
    except OSError:
        return False


def kill_strays():
    """Kills and reaps every process still parented to this one. Called once a tool has
    exited, when this process has no other children left; returns how many there were."""
    if PLATFORM != "linux":
        return 0
    killed, me = 0, os.getpid()
    for _ in range(64):
        found = []
        for entry in os.scandir("/proc"):
            if not entry.name.isdigit():
                continue
            try:
                with open(f"/proc/{entry.name}/stat", "rb") as handle:
                    fields = handle.read().rsplit(b")", 1)[1].split()
            except (OSError, IndexError):
                continue
            if len(fields) > 1 and int(fields[1]) == me:
                found.append(int(entry.name))
        if not found:
            return killed
        for pid in found:
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            try:
                os.waitpid(pid, 0)
            except ChildProcessError:
                pass
            killed += 1
    return killed


# --- the wrapper's side -------------------------------------------------------------------

def _libc():
    libc = ctypes.CDLL(None, use_errno=True)
    libc.syscall.restype = ctypes.c_long
    libc.prctl.restype = ctypes.c_int
    return libc


def _call(function, *args):
    result = function(*args)
    if result < 0:
        number = ctypes.get_errno()
        raise OSError(number, os.strerror(number))
    return result


def landlock_abi(libc=None):
    """The kernel's Landlock ABI version, or an OSError saying why there is none."""
    libc = libc or _libc()
    return _call(libc.syscall, ctypes.c_long(SYS_CREATE_RULESET), ctypes.c_void_p(None), ctypes.c_size_t(0),
                 ctypes.c_uint32(CREATE_RULESET_VERSION))


def no_new_privs(libc):
    _call(libc.prctl, ctypes.c_int(PR_SET_NO_NEW_PRIVS), ctypes.c_ulong(1), ctypes.c_ulong(0),
          ctypes.c_ulong(0), ctypes.c_ulong(0))


def apply_landlock(libc, abi, *, read=(), run=(), write=(), devices=()):
    """Confines this process (and what it executes) to the given paths."""
    fs = handled_fs(abi)
    attr = struct.pack("=QQQ", fs, NET_BIND_TCP | NET_CONNECT_TCP if abi >= 4 else 0,
                       SCOPE_ABSTRACT_UNIX_SOCKET | SCOPE_SIGNAL if abi >= 6 else 0)
    size = 24 if abi >= 6 else 16 if abi >= 4 else 8
    buffer = ctypes.create_string_buffer(attr, len(attr))
    ruleset = _call(libc.syscall, ctypes.c_long(SYS_CREATE_RULESET), buffer, ctypes.c_size_t(size), ctypes.c_uint32(0))
    try:
        for rights, paths in ((READ, read), (RUN, run), (WRITE, write), (DEVICE, devices)):
            for path in paths:
                try:
                    fd = os.open(path, os.O_PATH | os.O_CLOEXEC)
                except FileNotFoundError:
                    continue
                try:
                    allowed = rights & fs
                    if not stat.S_ISDIR(os.fstat(fd).st_mode):
                        allowed &= FILE_RIGHTS
                    rule = ctypes.create_string_buffer(struct.pack("=Qi", allowed, fd), 12)
                    _call(libc.syscall, ctypes.c_long(SYS_ADD_RULE), ctypes.c_int(ruleset),
                          ctypes.c_int(RULE_PATH_BENEATH), rule, ctypes.c_uint32(0))
                finally:
                    os.close(fd)
        _call(libc.syscall, ctypes.c_long(SYS_RESTRICT_SELF), ctypes.c_int(ruleset), ctypes.c_uint32(0))
    finally:
        os.close(ruleset)


def seccomp_program(machine=None):
    """The classic BPF filter for this processor, as packed sock_filter entries."""
    machine = {"amd64": "x86_64", "arm64": "aarch64"}.get((machine or platform.machine()).lower(),
                                                          (machine or platform.machine()).lower())
    if machine not in SECCOMP:
        raise OSError(errno.ENOTSUP, f"no system call filter for {machine} processors")
    audit, refused = SECCOMP[machine]
    errors = sorted(set(refused.values()))
    # Layout: arch check, load nr, [x32 check], one test per refused call, allow, one return
    # per error, kill. Jumps are relative to the next instruction.
    tests = sorted(refused)
    head = 3 + (machine == "x86_64")
    allow = head + len(tests)
    kill = allow + 1 + len(errors)
    program = [(LD_ABS_W, 0, 0, 4), (JEQ_K, 0, kill - 2, audit), (LD_ABS_W, 0, 0, 0)]
    if machine == "x86_64":
        program.append((JGE_K, kill - 4, 0, X32))
    for index, number in enumerate(tests):
        at = head + index
        program.append((JEQ_K, allow + 1 + errors.index(refused[number]) - at - 1, 0, number))
    program.append((RET_K, 0, 0, RET_ALLOW))
    program += [(RET_K, 0, 0, RET_ERRNO | e) for e in errors]
    program.append((RET_K, 0, 0, RET_KILL_PROCESS))
    return b"".join(struct.pack("=HBBI", *entry) for entry in program), len(program)


def apply_seccomp(libc):
    code, count = seccomp_program()
    filters = ctypes.create_string_buffer(code, len(code))

    class Program(ctypes.Structure):
        _fields_ = [("len", ctypes.c_ushort), ("filter", ctypes.c_void_p)]
    program = Program(count, ctypes.addressof(filters))
    _call(libc.prctl, ctypes.c_int(PR_SET_SECCOMP), ctypes.c_ulong(SECCOMP_MODE_FILTER),
          ctypes.c_void_p(ctypes.addressof(program)), ctypes.c_ulong(0), ctypes.c_ulong(0))


def apply_limits(limits):
    for name, value in limits.items():
        kind = RLIMITS[name]
        hard = resource.getrlimit(kind)[1]
        # CPU time: SIGXCPU at the soft limit, SIGKILL a minute later.
        wanted = (value, value + 60 if name == "cpu" else value)
        if hard != resource.RLIM_INFINITY:
            wanted = tuple(min(v, hard) for v in wanted)
        resource.setrlimit(kind, wanted)


def run_probe():
    """Confines this (throwaway) process and checks that the confinement holds."""
    import socket  # before Landlock hides the standard library
    out, reasons = {"landlock": None, "seccomp": False}, []
    try:
        libc = _libc()
        no_new_privs(libc)
    except OSError as error:
        print(json.dumps({**out, "reasons": [f"no_new_privs failed: {error.strerror}"]}))
        return
    try:
        abi = landlock_abi(libc)
        if abi < MIN_ABI:
            reasons.append(f"Landlock ABI {abi} is older than {MIN_ABI} (Linux 6.2)")
        else:
            apply_landlock(libc, abi, read=["/proc/self"])
            try:
                os.listdir("/")
                reasons.append("Landlock did not take effect")
            except PermissionError:
                out["landlock"] = abi
    except OSError as error:
        reasons.append({errno.ENOSYS: "the kernel has no Landlock (Linux 5.13 or later)",
                        errno.EOPNOTSUPP: "Landlock is turned off in this kernel (lsm= boot setting)",
                        errno.EPERM: "Landlock is blocked here, as by a container's seccomp profile"}.get(
                            error.errno, f"Landlock failed: {error.strerror}"))
    try:
        apply_seccomp(libc)
        try:
            socket.socket(socket.AF_INET, socket.SOCK_STREAM).close()
            reasons.append("the system call filter did not take effect")
        except PermissionError:
            out["seccomp"] = True
    except OSError as error:
        reasons.append(f"seccomp failed: {error.strerror}")
    print(json.dumps({**out, "reasons": reasons}))


def main(argv):
    if argv[1:] == ["--probe"]:
        run_probe()
        return 0
    if len(argv) < 5 or argv[1] != "--spec" or argv[3] != "--":
        print("usage: sandbox.py --spec JSON -- PROGRAM [ARG...] | sandbox.py --probe", file=sys.stderr)
        return 2
    spec, target = json.loads(argv[2]), argv[4:]
    try:
        apply_limits(spec.get("limits", {}))
        libc = _libc()
        no_new_privs(libc)
        if spec.get("landlock"):
            apply_landlock(libc, landlock_abi(libc), read=spec["read"], run=spec["run"], write=spec["write"],
                           devices=spec["devices"])
        if spec.get("seccomp"):
            apply_seccomp(libc)
    except OSError as error:
        print(f"kiwi-sandbox: could not confine {Path(target[0]).name}: {error}", file=sys.stderr)
        return 125
    try:
        os.execv(target[0], target)
    except OSError as error:
        print(f"kiwi-sandbox: could not start {Path(target[0]).name}: {error}", file=sys.stderr)
        return 127


if __name__ == "__main__":
    sys.exit(main(sys.argv))
