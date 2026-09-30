pub mod keyboard;

pub use keyboard::KeyboardController;

use std::sync::mpsc;

/// Work to run against the keyboard on the input thread.
pub type InputJob = Box<dyn FnOnce(&mut KeyboardController) + Send>;

/// Cloneable handle for the console and web clients. Jobs run one at a time, in send order.
pub type InputSender = mpsc::Sender<InputJob>;

/// Moves the keyboard onto its own thread so every source shares one ordered queue.
pub fn spawn_input_thread(mut keyboard: KeyboardController) -> InputSender {
    let (tx, rx) = mpsc::channel::<InputJob>();
    std::thread::spawn(move || {
        for job in rx {
            job(&mut keyboard);
        }
    });
    tx
}
