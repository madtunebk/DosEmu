#!/usr/bin/env python3
import json
import os
import socket
import time
import tkinter as tk
from collections import deque

from funcs import KeyboardController

QMP_SOCKET = f"{os.environ.get('XDG_RUNTIME_DIR', '/tmp')}/qemu-dos-{os.getuid()}/qmp.sock"
FRAME_FILE = "/dev/shm/qemu-dos-frame.ppm"
FPS = 10
INTERVAL_MS = 1000 // FPS
KEY_DELAY_MS = 45
KEY_HOLD_MS = 25


class QMPClient:
    def __init__(self, path: str):
        deadline = time.monotonic() + 15
        last_error = None
        while True:
            try:
                self.socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                self.socket.settimeout(2)
                self.socket.connect(path)
                self.reader = self.socket.makefile("r", encoding="utf-8")
                greeting = self._read_message()
                if "QMP" not in greeting:
                    raise RuntimeError("Invalid QMP greeting")
                self.execute("qmp_capabilities")
                return
            except (FileNotFoundError, ConnectionResetError, ConnectionRefusedError, OSError) as exc:
                last_error = exc
                if time.monotonic() >= deadline:
                    raise RuntimeError(f"QMP not ready at {path}: {exc}") from exc
                time.sleep(0.2)

    def _read_message(self):
        while True:
            line = self.reader.readline()
            if not line:
                raise ConnectionError("QMP connection closed")
            message = json.loads(line)
            if "event" not in message:
                return message

    def execute(self, command: str, arguments=None):
        payload = {"execute": command}
        if arguments is not None:
            payload["arguments"] = arguments
        self.socket.sendall(json.dumps(payload).encode() + b"\n")
        while True:
            response = self._read_message()
            if "error" in response:
                raise RuntimeError(response["error"])
            if "return" in response:
                return response["return"]

    def send_key(self, key: str):
        self.execute("human-monitor-command", {
            "command-line": f"sendkey {key} {KEY_HOLD_MS}"})

    def close(self):
        self.reader.close()
        self.socket.close()


def character_to_qemu_key(c: str):
    if "a" <= c <= "z" or "0" <= c <= "9":
        return c
    if "A" <= c <= "Z":
        return f"shift-{c.lower()}"
    return {
        " ": "spc", "\n": "ret", "\r": "ret", "\b": "backspace",
        ".": "dot", ",": "comma", "-": "minus", "_": "shift-minus",
        "=": "equal", "+": "shift-equal", "/": "slash", "\\": "backslash",
        ";": "semicolon", ":": "shift-semicolon", "'": "apostrophe",
        '"': "shift-apostrophe", "!": "shift-1", "@": "shift-2",
        "#": "shift-3", "$": "shift-4", "%": "shift-5", "^": "shift-6",
        "&": "shift-7", "*": "shift-8", "(": "shift-9", ")": "shift-0",
    }.get(c)


class DOSViewer:
    def __init__(self):
        if not os.path.exists(QMP_SOCKET):
            raise FileNotFoundError(
                f"QMP socket not found at {QMP_SOCKET}. Start the VM first with:\n"
                "  cargo run --release -- images/FD14FULL.img"
            )

        self.root = tk.Tk()
        self.root.title("Coaba DOS Lab — sanity viewer")
        self.root.configure(background="#111111")
        self.photo = None
        self.closed = False
        self.key_queue = deque()
        self.qmp = QMPClient(QMP_SOCKET)

        self.image_label = tk.Label(self.root, background="black", borderwidth=0,
                                    highlightthickness=0, takefocus=True)
        self.image_label.pack(expand=True, fill="both")
        self.status = tk.Label(self.root, text="Connected", foreground="#00ff66",
                               background="#111111", anchor="w", padx=8, pady=4)
        self.status.pack(fill="x")
        bar = tk.Frame(self.root, background="#1a1a1a", padx=8, pady=8)
        bar.pack(fill="x")
        self.command_entry = tk.Entry(bar, font=("Monospace", 12), background="#050505",
                                      foreground="#00ff66", insertbackground="#00ff66")
        self.command_entry.pack(side="left", expand=True, fill="x", padx=(0, 8), ipady=5)
        tk.Button(bar, text="Type", command=self.type_text).pack(side="left", padx=2)
        tk.Button(bar, text="Run ↵", command=self.run_command).pack(side="left", padx=2)
        tk.Button(bar, text="⌫", command=lambda: self.queue_key("backspace")).pack(side="left", padx=2)
        tk.Button(bar, text="Enter", command=lambda: self.queue_key("ret")).pack(side="left", padx=2)
        self.command_entry.bind("<Return>", lambda _e: self.run_command())
        self.image_label.bind("<Button-1>", lambda _e: self.image_label.focus_set())
        self.keyboard = KeyboardController(self.root, self.qmp,
                                           ignored_widget=self.command_entry,
                                           status_widget=self.status)
        self.root.protocol("WM_DELETE_WINDOW", self.close)
        self.root.after(0, self.update_frame)
        self.root.after(0, self.process_key_queue)
        self.command_entry.focus_set()

    def queue_key(self, key): self.key_queue.append(key)

    def queue_text(self, text):
        for c in text:
            key = character_to_qemu_key(c)
            if key:
                self.key_queue.append(key)

    def type_text(self):
        self.queue_text(self.command_entry.get())
        self.command_entry.delete(0, tk.END)

    def run_command(self):
        self.type_text()
        self.queue_key("ret")

    def process_key_queue(self):
        if self.closed:
            return
        if self.key_queue:
            try:
                self.qmp.send_key(self.key_queue.popleft())
            except Exception as error:
                self.status.configure(text=f"Input error: {error}", foreground="#ff5555")
        self.root.after(KEY_DELAY_MS, self.process_key_queue)

    def update_frame(self):
        if self.closed:
            return
        try:
            self.qmp.execute("screendump", {"filename": FRAME_FILE})
            self.photo = tk.PhotoImage(file=FRAME_FILE)
            self.image_label.configure(image=self.photo)
            if not self.key_queue:
                self.status.configure(text=f"LIVE • {self.photo.width()}×{self.photo.height()} • {FPS} FPS polling",
                                      foreground="#00ff66")
        except Exception as error:
            self.status.configure(text=f"Video error: {error}", foreground="#ff5555")
        self.root.after(INTERVAL_MS, self.update_frame)

    def close(self):
        self.closed = True
        self.keyboard.release_all()
        try:
            self.qmp.close()
        finally:
            try: os.unlink(FRAME_FILE)
            except FileNotFoundError: pass
            self.root.destroy()

    def run(self): self.root.mainloop()


if __name__ == "__main__":
    try:
        DOSViewer().run()
    except FileNotFoundError as exc:
        print(f"\n{exc}\n", file=os.sys.stderr)
        raise SystemExit(1)
