"""Measure editor-internal key latency through a pty (no terminal emulator involved).

Usage: python3 bench/latency.py FILE [--startup] [--idle SECONDS] [vim] [hx] [nib]

--startup measures only startup, for editors that cannot edit yet.
--idle also measures CPU use and wakeups while the editor waits for keys.
nib is run from target/release, so build it with `cargo build --release`.

Memory and idle CPU come from proc_pid_rusage, so only on macOS.
docs/benchmarks.md lists the files and the results.
"""
import ctypes, fcntl, os, pty, select, statistics, struct, sys, termios, time

ROWS, COLS = 50, 160
REPLIES = {
    b"\x1b[c": b"\x1b[?62;22c",
    b"\x1b[0c": b"\x1b[?62;22c",
    b"\x1b[>c": b"\x1b[>0;100;0c",
    b"\x1b[6n": b"\x1b[1;1R",
    b"\x1b]11;?": b"\x1b]11;rgb:0000/0000/0000\x1b\\",
    b"\x1b]10;?": b"\x1b]10;rgb:ffff/ffff/ffff\x1b\\",
}


def spawn(argv):
    pid, fd = pty.fork()
    if pid == 0:
        # Set the size before exec, so the editor never sees a 0x0 terminal.
        fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
        os.environ["TERM"] = "xterm-256color"
        os.execvp(argv[0], argv)
    return pid, fd


def answer(fd, data):
    for q, r in REPLIES.items():
        if q in data:
            os.write(fd, r)


def read_burst(fd, first_timeout, silence):
    """Return (t_first, t_last) relative to call, reading until `silence` s of no output."""
    t0 = time.perf_counter()
    t_first = t_last = None
    timeout = first_timeout
    while True:
        r, _, _ = select.select([fd], [], [], timeout)
        if not r:
            return t_first, t_last
        data = os.read(fd, 65536)
        now = time.perf_counter() - t0
        answer(fd, data)
        if t_first is None:
            t_first = now
        t_last = now
        timeout = silence


class RusageInfo(ctypes.Structure):
    """rusage_info_v0 from <libproc.h>."""
    _fields_ = [("uuid", ctypes.c_uint8 * 16)] + [(n, ctypes.c_uint64) for n in (
        "user_time", "system_time", "pkg_idle_wkups", "interrupt_wkups", "pageins",
        "wired_size", "resident_size", "phys_footprint", "proc_start_abstime", "proc_exit_abstime")]


def usage(pid):
    """(CPU seconds, wakeups, memory footprint in MB), or None off macOS."""
    if sys.platform != "darwin":
        return None
    libc = ctypes.CDLL("libSystem.dylib")
    info = RusageInfo()
    if libc.proc_pid_rusage(pid, 0, ctypes.byref(info)) != 0:
        return None
    # The times are in mach ticks, which are not nanoseconds on Apple silicon.
    timebase = (ctypes.c_uint32 * 2)()
    libc.mach_timebase_info(timebase)
    ticks = (info.user_time + info.system_time) * timebase[0] / timebase[1]
    return ticks / 1e9, info.pkg_idle_wkups + info.interrupt_wkups, info.phys_footprint / 2**20


def pct(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(len(xs) * p))]


def run(name, argv, enter_insert, startup_only, idle, n=300):
    # startup: spawn -> the output settles, including redraws that color
    # the screen after the first frame
    starts, memory = [], []
    for _ in range(10):
        pid, fd = spawn(argv)
        _, last = read_burst(fd, 5.0, 0.3)
        starts.append(last)
        if u := usage(pid):
            memory.append(u[2])
        os.kill(pid, 9); os.waitpid(pid, 0); os.close(fd)

    print(f"## {name}")
    print(f"  startup (until drawing settles): median {statistics.median(starts)*1000:.1f} ms")
    if memory:
        print(f"  memory after startup: median {statistics.median(memory):.1f} MB")
    if startup_only:
        return

    pid, fd = spawn(argv)
    read_burst(fd, 5.0, 0.5)
    if idle and (before := usage(pid)):
        # Nothing to read while idle, unless the editor draws on its own.
        read_burst(fd, idle, idle)
        after = usage(pid)
        cpu = (after[0] - before[0]) / idle * 100
        print(f"  idle {idle:g} s: CPU {cpu:.3f} % of a core, {(after[1] - before[1]) / idle:.1f} wakeups/s")
    results = {}
    for label, prep, key in [
        ("insert 'a'", enter_insert, b"a"),
        ("scroll C-d", b"\x1b", b"\x04"),
    ]:
        os.write(fd, prep)
        read_burst(fd, 0.5, 0.2)
        firsts, lasts = [], []
        for i in range(n):
            if label.startswith("scroll") and i % 40 == 0:
                os.write(fd, b"gg"); read_burst(fd, 0.5, 0.1)
            os.write(fd, key)
            f, l = read_burst(fd, 1.0, 0.015)
            if f is not None:
                firsts.append(f * 1000); lasts.append(l * 1000)
            time.sleep(0.01)
        results[label] = (firsts, lasts)
    if u := usage(pid):
        print(f"  memory after {2 * n} keys: {u[2]:.1f} MB")
    os.kill(pid, 9); os.waitpid(pid, 0); os.close(fd)

    for label, (firsts, lasts) in results.items():
        if not firsts:
            print(f"  {label:11s} no response")
            continue
        print(f"  {label:11s} first byte: median {statistics.median(firsts):.2f} ms  p99 {pct(firsts, .99):.2f} ms"
              f" | frame done: median {statistics.median(lasts):.2f} ms  p99 {pct(lasts, .99):.2f} ms  (n={len(firsts)})")


HX_CONFIG = b"[editor.lsp]\nenable = false\n"

EDITORS = {
    "vim": lambda f, tmp: ("vim --clean", ["vim", "--clean", f]),
    "hx": lambda f, tmp: ("helix (lsp off)", ["hx", "-c", tmp, f]),
    "nib": lambda f, tmp: ("nib", [NIB, f]),
}

NIB = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target", "release", "nib")


if __name__ == "__main__":
    import tempfile
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    args = sys.argv[2:]
    startup_only = "--startup" in args
    idle = 0.0
    if "--idle" in args:
        i = args.index("--idle")
        idle = float(args[i + 1])
        del args[i:i + 2]
    path, which = sys.argv[1], [a for a in args if a != "--startup"] or list(EDITORS)
    with tempfile.NamedTemporaryFile(suffix=".toml") as cfg:
        cfg.write(HX_CONFIG); cfg.flush()
        for e in which:
            name, argv = EDITORS[e](path, cfg.name)
            run(name, argv, b"i", startup_only, idle)
