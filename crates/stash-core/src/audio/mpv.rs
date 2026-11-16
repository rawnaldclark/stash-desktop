//! libmpv, loaded at run time.
//!
//! The library is opened from a file instead of being linked in: Stash starts (and says so) when the audio engine is
//! missing, and the engine can be swapped for another build without rebuilding Stash. Only the dozen functions of
//! mpv's client API that the engine uses are bound, by hand, against `client.h` of client API 2.x.

use std::ffi::{CStr, CString, c_char, c_int, c_ulong, c_void};
use std::path::Path;

use crate::{Error, Result};

#[repr(C)]
struct RawEvent {
    event_id: c_int,
    error: c_int,
    reply_userdata: u64,
    data: *mut c_void,
}

#[repr(C)]
struct RawProperty {
    name: *const c_char,
    format: c_int,
    data: *mut c_void,
}

#[repr(C)]
struct RawLog {
    prefix: *const c_char,
    level: *const c_char,
    text: *const c_char,
    log_level: c_int,
}

#[repr(C)]
struct RawEndFile {
    reason: c_int,
    error: c_int,
    playlist_entry_id: i64,
    playlist_insert_id: i64,
    playlist_insert_num_entries: c_int,
}

#[repr(C)]
struct RawHook {
    name: *const c_char,
    id: u64,
}

const FORMAT_NONE: c_int = 0;
const FORMAT_STRING: c_int = 1;

type Handle = *mut c_void;

/// The functions of `client.h` in use. Plain pointers: they stay valid for as long as the library is loaded, and the
/// library is a field of [`Mpv`] dropped after the handle.
struct Api {
    create: unsafe extern "C" fn() -> Handle,
    initialize: unsafe extern "C" fn(Handle) -> c_int,
    terminate_destroy: unsafe extern "C" fn(Handle),
    set_option_string: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> c_int,
    set_property_string: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> c_int,
    get_property_string: unsafe extern "C" fn(Handle, *const c_char) -> *mut c_char,
    free: unsafe extern "C" fn(*mut c_void),
    command: unsafe extern "C" fn(Handle, *const *const c_char) -> c_int,
    observe_property: unsafe extern "C" fn(Handle, u64, *const c_char, c_int) -> c_int,
    wait_event: unsafe extern "C" fn(Handle, f64) -> *mut RawEvent,
    request_log_messages: unsafe extern "C" fn(Handle, *const c_char) -> c_int,
    error_string: unsafe extern "C" fn(c_int) -> *const c_char,
    client_api_version: unsafe extern "C" fn() -> c_ulong,
    hook_add: unsafe extern "C" fn(Handle, u64, *const c_char, c_int) -> c_int,
    hook_continue: unsafe extern "C" fn(Handle, u64) -> c_int,
}

/// Why a file stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    /// It played to its end.
    Eof,
    /// It was stopped or replaced.
    Stop,
    Quit,
    /// It could not be opened or decoded; the number is mpv's error code.
    Error(i32),
    Redirect,
    Other(i32),
}

/// What the engine reports while it runs.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Nothing happened within the wait.
    None,
    Shutdown,
    Log {
        prefix: String,
        level: String,
        text: String,
    },
    StartFile,
    FileLoaded,
    EndFile(EndReason),
    /// The audio output or its filters were set up again (a new format, a new device, a new filter list).
    AudioReconfig,
    PlaybackRestart,
    Seek,
    /// An observed property changed; `None` when it has no value right now.
    Property {
        name: String,
        value: Option<String>,
    },
    Idle,
    /// A hook asked for with [`Mpv::hook_add`] is reached: mpv waits until [`Mpv::hook_continue`] is called with
    /// `id`.
    Hook {
        name: String,
        id: u64,
    },
    Other(i32),
}

/// One mpv player.
pub struct Mpv {
    handle: Handle,
    api: Api,
    // Dropped last: the function pointers above point into it.
    _lib: libloading::Library,
}

// mpv's client API is thread-safe: a handle may be used from any thread (client.h, "Threads").
unsafe impl Send for Mpv {}
// The same holds across threads at once: commands and properties may be used from any thread while another waits
// for events. Only one thread may wait for events at a time; the player keeps that to its own thread.
unsafe impl Sync for Mpv {}

fn audio(message: impl Into<String>) -> Error {
    Error::Audio(message.into())
}

fn c(text: &str) -> Result<CString> {
    CString::new(text).map_err(|_| audio(format!("a NUL byte in {text:?}")))
}

unsafe fn text(ptr: *const c_char) -> String {
    if ptr.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    }
}

impl Mpv {
    /// Loads libmpv from `library` (`libmpv-2.dll`, `libmpv.so.2`, `libmpv.2.dylib`), sets `options` and starts the
    /// player. Options are mpv's own, by name. Log lines of `log_level` and above arrive as [`Event::Log`].
    pub fn new(library: &Path, options: &[(&str, &str)], log_level: &str) -> Result<Self> {
        // Safety: loading a library runs its initialisers; the path is one the caller chose as the audio engine.
        let lib = unsafe { libloading::Library::new(library) }.map_err(|e| {
            audio(format!(
                "cannot load the audio engine at {}: {e}",
                library.display()
            ))
        })?;
        macro_rules! bind {
            ($name:literal) => {
                // Safety: the symbol's type is the one client.h declares for it.
                *unsafe { lib.get(concat!($name, "\0").as_bytes()) }.map_err(|e| {
                    audio(format!(
                        "{} is not libmpv ({} is missing: {e})",
                        library.display(),
                        $name
                    ))
                })?
            };
        }
        let api = Api {
            create: bind!("mpv_create"),
            initialize: bind!("mpv_initialize"),
            terminate_destroy: bind!("mpv_terminate_destroy"),
            set_option_string: bind!("mpv_set_option_string"),
            set_property_string: bind!("mpv_set_property_string"),
            get_property_string: bind!("mpv_get_property_string"),
            free: bind!("mpv_free"),
            command: bind!("mpv_command"),
            observe_property: bind!("mpv_observe_property"),
            wait_event: bind!("mpv_wait_event"),
            request_log_messages: bind!("mpv_request_log_messages"),
            error_string: bind!("mpv_error_string"),
            client_api_version: bind!("mpv_client_api_version"),
            hook_add: bind!("mpv_hook_add"),
            hook_continue: bind!("mpv_hook_continue"),
        };
        // The structs above are client API 2.x's.
        let version = unsafe { (api.client_api_version)() } as u64;
        if version >> 16 != 2 {
            return Err(audio(format!(
                "libmpv client API {}.{} is not supported (2.x is)",
                version >> 16,
                version & 0xffff
            )));
        }
        let handle = unsafe { (api.create)() };
        if handle.is_null() {
            return Err(audio("libmpv could not create a player"));
        }
        let mpv = Mpv {
            handle,
            api,
            _lib: lib,
        };
        for (name, value) in options {
            let (n, v) = (c(name)?, c(value)?);
            mpv.check(
                unsafe { (mpv.api.set_option_string)(mpv.handle, n.as_ptr(), v.as_ptr()) },
                &format!("option {name}={value}"),
            )?;
        }
        let level = c(log_level)?;
        mpv.check(
            unsafe { (mpv.api.request_log_messages)(mpv.handle, level.as_ptr()) },
            "log level",
        )?;
        mpv.check(
            unsafe { (mpv.api.initialize)(mpv.handle) },
            "starting the player",
        )?;
        Ok(mpv)
    }

    fn check(&self, code: c_int, what: &str) -> Result<()> {
        if code >= 0 {
            return Ok(());
        }
        let why = unsafe { text((self.api.error_string)(code)) };
        Err(audio(format!("{what}: {why}")))
    }

    /// Runs an mpv command, e.g. `["loadfile", path, "append-play"]`.
    pub fn command(&self, args: &[&str]) -> Result<()> {
        let owned: Vec<CString> = args.iter().map(|a| c(a)).collect::<Result<_>>()?;
        let mut pointers: Vec<*const c_char> = owned.iter().map(|a| a.as_ptr()).collect();
        pointers.push(std::ptr::null());
        self.check(
            unsafe { (self.api.command)(self.handle, pointers.as_ptr()) },
            &format!("command {}", args.join(" ")),
        )
    }

    pub fn set(&self, name: &str, value: &str) -> Result<()> {
        let (n, v) = (c(name)?, c(value)?);
        self.check(
            unsafe { (self.api.set_property_string)(self.handle, n.as_ptr(), v.as_ptr()) },
            &format!("property {name}={value}"),
        )
    }

    /// A property as mpv prints it, or `None` when it has no value right now.
    pub fn get(&self, name: &str) -> Option<String> {
        let n = c(name).ok()?;
        let ptr = unsafe { (self.api.get_property_string)(self.handle, n.as_ptr()) };
        if ptr.is_null() {
            return None;
        }
        let value = unsafe { text(ptr) };
        unsafe { (self.api.free)(ptr.cast()) };
        Some(value)
    }

    /// Asks for an [`Event::Property`] whenever `name` changes (and once now).
    pub fn observe(&self, name: &str) -> Result<()> {
        let n = c(name)?;
        self.check(
            unsafe { (self.api.observe_property)(self.handle, 0, n.as_ptr(), FORMAT_STRING) },
            &format!("observing {name}"),
        )
    }

    /// Asks mpv to stop at the hook `name` ("on_preloaded": a file is opened and its filters are about to be built)
    /// and send [`Event::Hook`]; it waits there until [`Mpv::hook_continue`]. Lower `priority` runs first.
    pub fn hook_add(&self, name: &str, priority: i32) -> Result<()> {
        let n = c(name)?;
        self.check(
            unsafe { (self.api.hook_add)(self.handle, 0, n.as_ptr(), priority) },
            &format!("hook {name}"),
        )
    }

    /// Lets mpv go on from the hook [`Event::Hook`] named by `id`.
    pub fn hook_continue(&self, id: u64) -> Result<()> {
        self.check(
            unsafe { (self.api.hook_continue)(self.handle, id) },
            "going on from a hook",
        )
    }

    /// mpv's own words for one of its error codes: "loading failed", "audio output initialization failed".
    pub fn error_text(&self, code: i32) -> String {
        unsafe { text((self.api.error_string)(code)) }
    }

    /// The next event, waiting up to `timeout_seconds` for one.
    pub fn wait_event(&self, timeout_seconds: f64) -> Event {
        // Safety: mpv returns a pointer to an event that stays valid until the next wait_event on this handle; it is
        // read out completely here, before returning.
        let raw = unsafe { &*(self.api.wait_event)(self.handle, timeout_seconds) };
        match raw.event_id {
            0 => Event::None,
            1 => Event::Shutdown,
            2 => {
                let log = unsafe { &*(raw.data as *const RawLog) };
                let _ = log.log_level;
                Event::Log {
                    prefix: unsafe { text(log.prefix) },
                    level: unsafe { text(log.level) },
                    text: unsafe { text(log.text) }.trim_end().to_string(),
                }
            }
            6 => Event::StartFile,
            7 => {
                let end = unsafe { &*(raw.data as *const RawEndFile) };
                let _ = (
                    end.playlist_entry_id,
                    end.playlist_insert_id,
                    end.playlist_insert_num_entries,
                );
                Event::EndFile(match end.reason {
                    0 => EndReason::Eof,
                    2 => EndReason::Stop,
                    3 => EndReason::Quit,
                    4 => EndReason::Error(end.error),
                    5 => EndReason::Redirect,
                    other => EndReason::Other(other),
                })
            }
            8 => Event::FileLoaded,
            11 => Event::Idle,
            18 => Event::AudioReconfig,
            20 => Event::Seek,
            21 => Event::PlaybackRestart,
            22 => {
                let property = unsafe { &*(raw.data as *const RawProperty) };
                let value = if property.format == FORMAT_STRING && !property.data.is_null() {
                    Some(unsafe { text(*(property.data as *const *const c_char)) })
                } else {
                    debug_assert!(
                        property.format == FORMAT_NONE || property.format == FORMAT_STRING
                    );
                    None
                };
                Event::Property {
                    name: unsafe { text(property.name) },
                    value,
                }
            }
            25 => {
                let hook = unsafe { &*(raw.data as *const RawHook) };
                Event::Hook {
                    name: unsafe { text(hook.name) },
                    id: hook.id,
                }
            }
            other => {
                let _ = (raw.error, raw.reply_userdata);
                Event::Other(other)
            }
        }
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        // Stops playback, closes the audio output (a file output is finished here) and frees the player.
        unsafe { (self.api.terminate_destroy)(self.handle) };
    }
}
