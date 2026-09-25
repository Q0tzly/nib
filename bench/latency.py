"""Measure editor-internal key latency through a pty (no terminal emulator involved).

Usage: python3 bench/latency.py FILE [--startup] [vim] [hx] [nib]

--startup measures only startup, for editors that cannot edit yet.
nib is run from target/release, so build it with `cargo build --release`.

docs/architecture.md uses ropey 1.6.1 src/rope.rs (3,455 lines) as FILE.
"""
import fcntl, os, pty, select, statistics, struct, sys, termios, time

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


def pct(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(len(xs) * p))]


def run(name, argv, enter_insert, startup_only, n=300):
    # startup: spawn -> end of first output burst
    starts = []
    for _ in range(10):
        pid, fd = spawn(argv)
        _, last = read_burst(fd, 5.0, 0.3)
        starts.append(last)
        os.kill(pid, 9); os.waitpid(pid, 0); os.close(fd)

    print(f"## {name}")
    print(f"  startup (to end of first frame): median {statistics.median(starts)*1000:.1f} ms")
    if startup_only:
        return

    pid, fd = spawn(argv)
    read_burst(fd, 5.0, 0.5)
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
    path, which = sys.argv[1], [a for a in args if a != "--startup"] or list(EDITORS)
    with tempfile.NamedTemporaryFile(suffix=".toml") as cfg:
        cfg.write(HX_CONFIG); cfg.flush()
        for e in which:
            name, argv = EDITORS[e](path, cfg.name)
            run(name, argv, b"i", startup_only)
