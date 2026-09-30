use serde_json::json;
use std::collections::HashSet;
use std::error::Error;
use std::thread;
use std::time::Duration;

use crate::session::SharedQmp;

// Same pacing as scripts/viewer.py: DOS drops keys that arrive faster than this.
const KEY_HOLD: Duration = Duration::from_millis(25);
const KEY_DELAY: Duration = Duration::from_millis(45);

/// Real-time keyboard over QMP `input-send-event` (QEMU qcodes).
pub struct KeyboardController {
    qmp: SharedQmp,
    held_keys: HashSet<String>,
}

impl KeyboardController {
    pub fn new(qmp: SharedQmp) -> Self {
        Self { qmp, held_keys: HashSet::new() }
    }

    fn send_key_event(&mut self, qcode: &str, down: bool) -> Result<(), Box<dyn Error>> {
        let args = json!({ "events": [{
            "type": "key",
            "data": { "down": down, "key": { "type": "qcode", "data": qcode } },
        }]});
        self.qmp.lock().unwrap().execute("input-send-event", Some(args))?;
        Ok(())
    }

    /// Ignored if the key is already held, so host autorepeat doesn't stack presses.
    pub fn key_down(&mut self, qcode: &str) -> Result<(), Box<dyn Error>> {
        if !self.held_keys.insert(qcode.to_string()) {
            return Ok(());
        }
        let result = self.send_key_event(qcode, true);
        if result.is_err() {
            self.held_keys.remove(qcode);
        }
        result
    }

    /// Typematic repeat: another key-down for a key that is already held.
    pub fn key_repeat(&mut self, qcode: &str) -> Result<(), Box<dyn Error>> {
        if !self.held_keys.contains(qcode) {
            return self.key_down(qcode);
        }
        self.send_key_event(qcode, true)
    }

    pub fn key_up(&mut self, qcode: &str) -> Result<(), Box<dyn Error>> {
        if !self.held_keys.remove(qcode) {
            return Ok(());
        }
        self.send_key_event(qcode, false)
    }

    pub fn release_all(&mut self) {
        for key in std::mem::take(&mut self.held_keys) {
            let _ = self.send_key_event(&key, false);
        }
    }

    /// Press and release a key, optionally with modifiers held (e.g. `&["ctrl", "alt"]`, "delete").
    pub fn tap_with(&mut self, modifiers: &[&str], qcode: &str) -> Result<(), Box<dyn Error>> {
        for modifier in modifiers {
            self.key_down(modifier)?;
        }
        self.key_down(qcode)?;
        thread::sleep(KEY_HOLD);
        self.key_up(qcode)?;
        for modifier in modifiers.iter().rev() {
            self.key_up(modifier)?;
        }
        thread::sleep(KEY_DELAY);
        Ok(())
    }

    pub fn tap(&mut self, qcode: &str) -> Result<(), Box<dyn Error>> {
        self.tap_with(&[], qcode)
    }

    /// Type text on a US layout. Returns the characters that have no mapping (they are skipped).
    pub fn type_text(&mut self, text: &str) -> Result<Vec<char>, Box<dyn Error>> {
        let mut skipped = Vec::new();
        for c in text.chars() {
            match char_to_qcode(c) {
                Some((true, qcode)) => self.tap_with(&["shift"], qcode)?,
                Some((false, qcode)) => self.tap(qcode)?,
                None => skipped.push(c),
            }
        }
        Ok(skipped)
    }
}

impl Drop for KeyboardController {
    fn drop(&mut self) {
        self.release_all();
    }
}

/// Maps a character to (needs shift, qcode) on a US-101 layout.
pub fn char_to_qcode(c: char) -> Option<(bool, &'static str)> {
    const LETTERS: [&str; 26] = [
        "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m",
        "n", "o", "p", "q", "r", "s", "t", "u", "v", "w", "x", "y", "z",
    ];
    const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];

    if c.is_ascii_lowercase() {
        return Some((false, LETTERS[(c as u8 - b'a') as usize]));
    }
    if c.is_ascii_uppercase() {
        return Some((true, LETTERS[(c as u8 - b'A') as usize]));
    }
    if c.is_ascii_digit() {
        return Some((false, DIGITS[(c as u8 - b'0') as usize]));
    }
    Some(match c {
        ' ' => (false, "spc"),
        '\n' | '\r' => (false, "ret"),
        '\t' => (false, "tab"),
        '\u{8}' => (false, "backspace"),
        '.' => (false, "dot"),
        ',' => (false, "comma"),
        '-' => (false, "minus"),
        '=' => (false, "equal"),
        '/' => (false, "slash"),
        '\\' => (false, "backslash"),
        ';' => (false, "semicolon"),
        '\'' => (false, "apostrophe"),
        '`' => (false, "grave_accent"),
        '[' => (false, "bracket_left"),
        ']' => (false, "bracket_right"),
        '_' => (true, "minus"),
        '+' => (true, "equal"),
        '?' => (true, "slash"),
        '|' => (true, "backslash"),
        ':' => (true, "semicolon"),
        '"' => (true, "apostrophe"),
        '~' => (true, "grave_accent"),
        '{' => (true, "bracket_left"),
        '}' => (true, "bracket_right"),
        '<' => (true, "comma"),
        '>' => (true, "dot"),
        '!' => (true, "1"),
        '@' => (true, "2"),
        '#' => (true, "3"),
        '$' => (true, "4"),
        '%' => (true, "5"),
        '^' => (true, "6"),
        '&' => (true, "7"),
        '*' => (true, "8"),
        '(' => (true, "9"),
        ')' => (true, "0"),
        _ => return None,
    })
}
