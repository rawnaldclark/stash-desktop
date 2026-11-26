//! A phone plugged in over USB ("File transfer"), through Windows Portable Devices (MTP): the phones connected, the
//! folder Stash for Android's "Send to computer" leaves (`Download/Stash transfer/`, with `stash-transfer.json`), its
//! files read to this computer and deleted from the phone once taken, and the folder once empty.
//!
//! Every call is blocking and uses COM on the calling thread (initialised here, multithreaded); a [`Transfer`] stays on
//! the thread that opened it. Other systems have no phones here.

/// A phone (or any portable device) Windows sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phone {
    /// Windows' id for it (its device path): what [`open_transfer`] takes.
    pub id: String,
    /// Its name, as Windows shows it ("Pixel 5").
    pub name: String,
}

/// A file in the phone's transfer folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneFile {
    pub object: String,
    pub name: String,
    pub size: u64,
}

/// The folder Stash for Android sends to: Internal storage › Download › Stash transfer.
pub const FOLDER: [&str; 2] = ["Download", "Stash transfer"];
/// The list of what each file is, written last by the phone.
pub const LIST: &str = "stash-transfer.json";
/// The phone's library sent with its songs (its likes, playlists and plays, as "Export for Stash on the web" writes
/// them, with the send's `sent_at_ms`): written after the songs and before the list. Neither a song nor the list.
pub const LIBRARY: &str = "stash-library.json";

/// A file that sends nothing for this long is taken as stuck, and its read is called off. A phone's File transfer was
/// seen to stop mid-file every hundred or so songs and never start again by itself, until it was unplugged.
pub const STALL: std::time::Duration = std::time::Duration::from_secs(20);
/// What a read called off for sending nothing says: the caller opens the phone again rather than give up on the song.
pub const STALLED: &str = "the phone stopped sending mid-song";
/// What trouble on this computer's side of a copy starts with when it is the drive's (the file could not be made,
/// written or kept: a full disk, a drive gone): not the phone's doing, so not a phone lost, and every song would meet
/// it.
pub const HERE: &str = "on this computer: ";
/// What trouble on this computer's side starts with when it is one song's alone (its file held by another program
/// past a few tries, say): not the phone's doing either, and that song is not taken while the rest are.
pub const ONE_SONG: &str = "for this song, on this computer: ";

/// Whether another program has the file open (Windows' sharing and lock violations): an antivirus or a sync client
/// looking at a file just written, which lets it go in a moment.
pub fn held_by_another(e: &std::io::Error) -> bool {
    cfg!(windows) && matches!(e.raw_os_error(), Some(32 | 33))
}

/// What a read tells as it goes: how much of the file has come so far, and (once) that the file has sent nothing for
/// `stall_after`, when its read is called off. `stalled` is told from another thread.
pub struct Watch<'a> {
    pub stall_after: std::time::Duration,
    pub progress: &'a (dyn Fn(u64) + Sync),
    pub stalled: &'a (dyn Fn() + Sync),
}

fn no_progress(_: u64) {}
fn no_stall() {}

impl Watch<'static> {
    /// Telling no one, called off after the usual [`STALL`].
    pub fn quiet() -> Self {
        Watch {
            stall_after: STALL,
            progress: &no_progress,
            stalled: &no_stall,
        }
    }
}

#[cfg(windows)]
pub use imp::{Transfer, open_transfer, phones};

#[cfg(not(windows))]
pub fn phones() -> Result<Vec<Phone>, String> {
    Ok(Vec::new())
}

#[cfg(windows)]
mod imp {
    use std::io::Write;
    use std::path::Path;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::mpsc::{RecvTimeoutError, channel};
    use std::time::{Duration, Instant};

    use windows::Win32::Devices::PortableDevices::*;
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
        CoTaskMemFree, CoUninitialize, IStream, STGM_READ,
    };
    use windows::Win32::System::Variant::VT_LPWSTR;
    use windows::core::{HSTRING, Interface, PCWSTR, PWSTR};

    use super::{FOLDER, HERE, ONE_SONG, Phone, PhoneFile, STALLED, Watch, held_by_another};

    fn com() {
        // Already initialised on this thread (in either mode) is fine.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    }

    /// A COM object handed to the thread that watches a read. Sound because every thread here is in the
    /// multithreaded apartment and the phone is opened free-threaded (`PortableDeviceFTM`), so its objects may be
    /// called from any of them; the watcher only ever calls `Cancel`.
    struct Agile<T>(T);
    unsafe impl<T> Send for Agile<T> {}
    unsafe impl<T> Sync for Agile<T> {}

    fn said(e: windows::core::Error) -> String {
        format!("the phone did not answer ({:#x})", e.code().0)
    }

    /// Takes a string COM handed over and frees it.
    unsafe fn taken(p: PWSTR) -> String {
        if p.is_null() {
            return String::new();
        }
        let s = unsafe { p.to_string() }.unwrap_or_default();
        unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
        s
    }

    /// The portable devices Windows has now.
    pub fn phones() -> Result<Vec<Phone>, String> {
        com();
        unsafe {
            let manager: IPortableDeviceManager =
                CoCreateInstance(&PortableDeviceManager, None, CLSCTX_INPROC_SERVER)
                    .map_err(said)?;
            let _ = manager.RefreshDeviceList();
            let mut count = 0u32;
            manager
                .GetDevices(std::ptr::null_mut(), &mut count)
                .map_err(said)?;
            if count == 0 {
                return Ok(Vec::new());
            }
            let mut ids = vec![PWSTR::null(); count as usize];
            manager
                .GetDevices(ids.as_mut_ptr(), &mut count)
                .map_err(said)?;
            let mut out = Vec::new();
            for raw in ids.into_iter().take(count as usize) {
                let id = taken(raw);
                let wide = HSTRING::from(id.as_str());
                let mut len = 0u32;
                let _ =
                    manager.GetDeviceFriendlyName(PCWSTR(wide.as_ptr()), PWSTR::null(), &mut len);
                let name = if len > 0 {
                    let mut buf = vec![0u16; len as usize];
                    let _ = manager.GetDeviceFriendlyName(
                        PCWSTR(wide.as_ptr()),
                        PWSTR(buf.as_mut_ptr()),
                        &mut len,
                    );
                    String::from_utf16_lossy(&buf)
                        .trim_end_matches('\0')
                        .to_string()
                } else {
                    String::new()
                };
                out.push(Phone {
                    id,
                    name: if name.trim().is_empty() {
                        "Your phone".into()
                    } else {
                        name
                    },
                });
            }
            Ok(out)
        }
    }

    /// A phone's transfer folder, open: its files, read and deleted through it.
    pub struct Transfer {
        /// Held for as long as the transfer is: the phone closes when the last reference to it goes.
        _device: IPortableDevice,
        content: IPortableDeviceContent,
        folder: String,
        pub files: Vec<PhoneFile>,
    }

    /// What the phone says when an object it listed is no longer there (deleted since: the phone clearing its copies
    /// as the computer lists them).
    fn gone(e: &windows::core::Error) -> bool {
        // HRESULT_FROM_WIN32(ERROR_NOT_FOUND), HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND)
        matches!(e.code().0 as u32, 0x8007_0490 | 0x8007_0002)
    }

    /// An object's name and size: Ok(None) for an object with no name, or one gone since it was listed; Err when the
    /// phone did not say (its properties could not be read), so a listing that left it out is not all of them.
    fn name_of(props: &IPortableDeviceProperties, object: &str) -> windows::core::Result<Option<(String, u64)>> {
        unsafe {
            let keys: IPortableDeviceKeyCollection =
                CoCreateInstance(&PortableDeviceKeyCollection, None, CLSCTX_INPROC_SERVER)?;
            for key in [
                &WPD_OBJECT_ORIGINAL_FILE_NAME,
                &WPD_OBJECT_NAME,
                &WPD_OBJECT_SIZE,
            ] {
                keys.Add(key as *const PROPERTYKEY)?;
            }
            let wide = HSTRING::from(object);
            let values = match props.GetValues(PCWSTR(wide.as_ptr()), &keys) {
                Ok(values) => values,
                Err(e) if gone(&e) => return Ok(None),
                Err(e) => return Err(e),
            };
            let text = |key: &PROPERTYKEY| {
                values
                    .GetStringValue(key)
                    .ok()
                    .map(|p| taken(p))
                    .filter(|s| !s.is_empty())
            };
            let Some(name) = text(&WPD_OBJECT_ORIGINAL_FILE_NAME).or_else(|| text(&WPD_OBJECT_NAME)) else {
                return Ok(None);
            };
            let size = values
                .GetUnsignedLargeIntegerValue(&WPD_OBJECT_SIZE)
                .unwrap_or(0);
            Ok(Some((name, size)))
        }
    }

    /// The objects inside `parent`, with their names and sizes, and whether that is all of them: a listing the phone
    /// stopped part way (it failed to say what comes next, or what one of them is) can leave any of them out.
    fn children(content: &IPortableDeviceContent, parent: &str) -> (Vec<PhoneFile>, bool) {
        let mut out = Vec::new();
        let mut all = true;
        unsafe {
            let Ok(props) = content.Properties() else {
                return (out, false);
            };
            let wide = HSTRING::from(parent);
            let Ok(found) = content.EnumObjects(0, PCWSTR(wide.as_ptr()), None) else {
                return (out, false);
            };
            loop {
                let mut ids = [PWSTR::null(); 32];
                let mut fetched = 0u32;
                let hr = found.Next(&mut ids, &mut fetched);
                for raw in ids.into_iter().take(fetched as usize) {
                    let object = taken(raw);
                    match name_of(&props, &object) {
                        Ok(Some((name, size))) => out.push(PhoneFile { object, name, size }),
                        Ok(None) => {}
                        Err(_) => all = false,
                    }
                }
                if hr.is_err() {
                    return (out, false);
                }
                // S_FALSE: fewer than asked for, the last of them.
                if fetched == 0 || hr.0 != 0 {
                    return (out, all);
                }
            }
        }
    }

    /// The transfer folder on `phone`, when it is there (in any of its storages), with every file in it; None when the
    /// phone has none, or shows no storage (it is charging only: "File transfer" is not chosen). A phone that stopped
    /// listing part way has not answered: what it listed can leave out any file, its list among them, so it is never
    /// taken to say the list is gone.
    pub fn open_transfer(phone: &str) -> Result<Option<Transfer>, String> {
        com();
        unsafe {
            let info: IPortableDeviceValues =
                CoCreateInstance(&PortableDeviceValues, None, CLSCTX_INPROC_SERVER)
                    .map_err(said)?;
            let _ = info.SetStringValue(&WPD_CLIENT_NAME, &HSTRING::from("Stash"));
            let _ = info.SetUnsignedIntegerValue(&WPD_CLIENT_MAJOR_VERSION, 1);
            let _ = info.SetUnsignedIntegerValue(&WPD_CLIENT_MINOR_VERSION, 0);
            let _ = info.SetUnsignedIntegerValue(&WPD_CLIENT_REVISION, 0);
            let device: IPortableDevice =
                CoCreateInstance(&PortableDeviceFTM, None, CLSCTX_INPROC_SERVER).map_err(said)?;
            device.Open(&HSTRING::from(phone), &info).map_err(said)?;
            let content = device.Content().map_err(said)?;
            let part_way = || Err("the phone stopped listing its files part way".to_string());
            let (storages, mut whole) = children(&content, "DEVICE");
            for storage in storages {
                let mut at = storage.object.clone();
                let mut found = true;
                for step in FOLDER {
                    let (inside, all) = children(&content, &at);
                    match inside.into_iter().find(|c| c.name.eq_ignore_ascii_case(step)) {
                        Some(next) => at = next.object,
                        None => {
                            whole &= all;
                            found = false;
                            break;
                        }
                    }
                }
                if found {
                    let (files, all) = children(&content, &at);
                    if !all {
                        return part_way();
                    }
                    return Ok(Some(Transfer {
                        _device: device,
                        content,
                        folder: at,
                        files,
                    }));
                }
            }
            if whole { Ok(None) } else { part_way() }
        }
    }

    impl Transfer {
        /// The whole of a small file (the list).
        pub fn bytes(&self, file: &PhoneFile, cap: u64) -> Result<Vec<u8>, String> {
            let mut out = Vec::new();
            self.each_block(file, &Watch::quiet(), &mut |block| {
                if out.len() as u64 + block.len() as u64 > cap {
                    return Err("the list is too big".into());
                }
                out.extend_from_slice(block);
                Ok(())
            })?;
            Ok(out)
        }

        /// Copies `file` to `to` (a new file), telling `watch` how much has come; how many bytes, once they are on the
        /// disk (not only in Windows' cache: the phone's copy may be deleted next). Nothing is left at `to` when it
        /// fails; one called off for sending nothing fails with [`STALLED`], and trouble writing here starts with
        /// [`HERE`] ([`ONE_SONG`] when another program holds `to`).
        pub fn copy(&self, file: &PhoneFile, to: &Path, watch: &Watch) -> Result<u64, String> {
            let here = |e: std::io::Error| {
                if held_by_another(&e) {
                    format!("{ONE_SONG}another program has the file open: {e}")
                } else {
                    format!("{HERE}{e}")
                }
            };
            let mut out = std::fs::File::create(to).map_err(here)?;
            let mut n = 0u64;
            let done = self.each_block(file, watch, &mut |block| {
                n += block.len() as u64;
                out.write_all(block).map_err(here)?;
                (watch.progress)(n);
                Ok(())
            });
            if let Err(e) = done.and_then(|_| out.sync_all().map_err(here)) {
                drop(out);
                let _ = std::fs::remove_file(to);
                return Err(e);
            }
            Ok(n)
        }

        /// Reads `file` to its end, a block at a time, into `take`. Another thread watches: when nothing has come for
        /// `watch.stall_after` (the phone stuck mid-file, which it does not get over by itself), it tells
        /// `watch.stalled` and calls the read off (the stream's and the transfer's `Cancel`), which then fails with
        /// [`STALLED`]. Should the read not end even so, `stalled` has been told, so the listener can be asked to
        /// turn File transfer off and on, which ends it.
        fn each_block(
            &self,
            file: &PhoneFile,
            watch: &Watch,
            take: &mut dyn FnMut(&[u8]) -> Result<(), String>,
        ) -> Result<(), String> {
            let started = Instant::now();
            let since = || started.elapsed().as_millis() as u64;
            // When something last came (ms after `started`); the file's stream, once open; whether it was called off.
            let heard = AtomicU64::new(0);
            let open: Mutex<Option<Agile<IPortableDeviceDataStream>>> = Mutex::new(None);
            let called_off = AtomicBool::new(false);
            let resources = unsafe { self.content.Transfer() }.map_err(said)?;
            let asked = Agile(resources.clone());
            let (finished, wait) = channel::<()>();
            std::thread::scope(|scope| {
                let (heard, open, called_off, asked) = (&heard, &open, &called_off, &asked);
                let quiet_for = watch.stall_after.as_millis() as u64;
                scope.spawn(move || {
                    let inited = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
                    // Ends as soon as the read does (its sender dropped), or once it has called the read off.
                    while let Err(RecvTimeoutError::Timeout) =
                        wait.recv_timeout(Duration::from_millis(250))
                    {
                        if since().saturating_sub(heard.load(Ordering::Relaxed)) >= quiet_for {
                            called_off.store(true, Ordering::SeqCst);
                            (watch.stalled)();
                            if let Some(stream) = open.lock().unwrap().as_ref() {
                                let _ = unsafe { stream.0.Cancel() };
                            }
                            let _ = unsafe { asked.0.Cancel() };
                            break;
                        }
                    }
                    if inited {
                        unsafe { CoUninitialize() };
                    }
                });
                let read = (|| {
                    let wide = HSTRING::from(file.object.as_str());
                    let mut optimal = 0u32;
                    let mut stream: Option<IStream> = None;
                    let got = unsafe {
                        resources.GetStream(
                            PCWSTR(wide.as_ptr()),
                            &WPD_RESOURCE_DEFAULT,
                            STGM_READ.0,
                            &mut optimal,
                            &mut stream,
                        )
                    };
                    if called_off.load(Ordering::SeqCst) {
                        return Err(STALLED.to_string());
                    }
                    got.map_err(said)?;
                    let stream = stream.ok_or_else(|| "the phone gave no file".to_string())?;
                    *open.lock().unwrap() = stream.cast::<IPortableDeviceDataStream>().ok().map(Agile);
                    let mut buf = vec![0u8; (optimal as usize).clamp(64 * 1024, 4 * 1024 * 1024)];
                    loop {
                        let mut read = 0u32;
                        let hr = unsafe {
                            stream.Read(
                                buf.as_mut_ptr() as *mut _,
                                buf.len() as u32,
                                Some(&mut read),
                            )
                        };
                        if called_off.load(Ordering::SeqCst) {
                            return Err(STALLED.to_string());
                        }
                        if hr.is_err() {
                            return Err(format!("the phone stopped sending ({:#x})", hr.0));
                        }
                        if read == 0 {
                            return Ok(());
                        }
                        heard.store(since(), Ordering::Relaxed);
                        take(&buf[..read as usize])?;
                        // Writing the block here is not the phone's silence.
                        heard.store(since(), Ordering::Relaxed);
                    }
                })();
                drop(finished);
                read
            })
        }

        /// Deletes the transfer folder from the phone when it is empty; fails, deleting nothing, when anything is in
        /// it (a file this computer has not seen: never deleted with it).
        pub fn remove_if_empty(&self) -> Result<(), String> {
            self.delete(&self.folder, PORTABLE_DEVICE_DELETE_NO_RECURSION)
        }

        /// Deletes one file of the transfer from the phone (one taken: the phone counts what is left).
        pub fn remove_file(&self, file: &PhoneFile) -> Result<(), String> {
            self.delete(&file.object, PORTABLE_DEVICE_DELETE_WITH_RECURSION)
        }

        fn delete(&self, object: &str, how: DELETE_OBJECT_OPTIONS) -> Result<(), String> {
            unsafe {
                let ids: IPortableDevicePropVariantCollection = CoCreateInstance(
                    &PortableDevicePropVariantCollection,
                    None,
                    CLSCTX_INPROC_SERVER,
                )
                .map_err(said)?;
                let wide = HSTRING::from(object);
                // One VT_LPWSTR naming the object. `Add` keeps its own copy, so this one is never cleared (the string
                // is `wide`'s, freed with it).
                let mut one: std::mem::ManuallyDrop<PROPVARIANT> =
                    std::mem::ManuallyDrop::new(std::mem::zeroed());
                let inner = &mut one.Anonymous.Anonymous;
                inner.vt = VT_LPWSTR;
                inner.Anonymous.pwszVal = PWSTR(wide.as_ptr() as *mut u16);
                ids.Add(&*one).map_err(said)?;
                self.content
                    .Delete(how.0 as u32, &ids, std::ptr::null_mut())
                    .map_err(said)
            }
        }
    }
}
