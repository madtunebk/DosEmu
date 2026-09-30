from __future__ import annotations

import tkinter as tk
from typing import Any


KEY_MAP = {
    "w": "w", "a": "a", "s": "s", "d": "d",
    "Up": "up", "Down": "down", "Left": "left", "Right": "right",
    "space": "spc", "Return": "ret", "Escape": "esc",
    "BackSpace": "backspace", "Tab": "tab",
    "Control_L": "ctrl", "Control_R": "ctrl",
    "Shift_L": "shift", "Shift_R": "shift",
    "Alt_L": "alt", "Alt_R": "alt",
    "Home": "home", "End": "end", "Prior": "pgup", "Next": "pgdn",
    "Insert": "insert", "Delete": "delete",
    **{f"F{i}": f"f{i}" for i in range(1, 13)},
}


class KeyboardController:
    def __init__(self, root: tk.Misc, qmp: Any, *, ignored_widget=None,
                 status_widget: tk.Label | None = None):
        self.root = root
        self.qmp = qmp
        self.ignored_widget = ignored_widget
        self.status_widget = status_widget
        self.held_keys: set[str] = set()
        root.bind_all("<KeyPress>", self.on_key_press, add="+")
        root.bind_all("<KeyRelease>", self.on_key_release, add="+")
        root.bind("<FocusOut>", self.on_focus_out, add="+")

    def translate_key(self, keysym: str) -> str | None:
        if keysym in KEY_MAP:
            return KEY_MAP[keysym]
        lowered = keysym.lower()
        return lowered if len(lowered) == 1 and lowered.isalnum() else None

    def send_key_event(self, key: str, down: bool):
        self.qmp.execute("input-send-event", {"events": [{
            "type": "key",
            "data": {"down": down, "key": {"type": "qcode", "data": key}},
        }]})

    def key_down(self, key: str):
        if key in self.held_keys:
            return
        self.held_keys.add(key)
        try:
            self.send_key_event(key, True)
        except Exception:
            self.held_keys.discard(key)
            raise

    def key_up(self, key: str):
        if key not in self.held_keys:
            return
        try:
            self.send_key_event(key, False)
        finally:
            self.held_keys.discard(key)

    def tap(self, key: str):
        self.send_key_event(key, True)
        self.send_key_event(key, False)

    def on_key_press(self, event):
        if event.widget == self.ignored_widget:
            return
        key = self.translate_key(event.keysym)
        if key:
            try:
                self.key_down(key)
            except Exception as error:
                self.set_error(f"Key-down failed: {error}")

    def on_key_release(self, event):
        if event.widget == self.ignored_widget:
            return
        key = self.translate_key(event.keysym)
        if key:
            try:
                self.key_up(key)
            except Exception as error:
                self.set_error(f"Key-up failed: {error}")

    def on_focus_out(self, _event=None):
        self.release_all()

    def release_all(self):
        for key in list(self.held_keys):
            try:
                self.send_key_event(key, False)
            except Exception:
                pass
        self.held_keys.clear()

    def set_error(self, message: str):
        if self.status_widget is not None:
            self.status_widget.configure(text=message, foreground="#ff5555")
