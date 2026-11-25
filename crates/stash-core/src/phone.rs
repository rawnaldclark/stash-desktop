//! A phone plugged in over USB ("File transfer"), through Windows Portable Devices (MTP): the phones connected, the
//! folder Stash for Android's "Send to computer" leaves (`Download/Stash transfer/`, with `stash-transfer.json`), its
//! files read to this computer, and the folder deleted once taken.
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

/// A file that sends nothing for this long is taken as stuck, and its read is called off. A phone's File transfer was
/// seen to stop mid-file every hundred or so songs and never start again by itself, until it was unplugged.
pub const STALL: std::time::Duration = std::time::Duration::from_secs(20);
/// What a read called off for sending nothing says: the caller opens the phone again rather than give up on the song.
pub const STALLED: &str = "the phone stopped sending mid-song";

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

    use super::{FOLDER, Phone, PhoneFile, STALLED, Watch};

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

    fn name_of(props: &IPortableDeviceProperties, object: &str) -> Option<(String, u64)> {
        unsafe {
            let keys: IPortableDeviceKeyCollection =
                CoCreateInstance(&PortableDeviceKeyCollection, None, CLSCTX_INPROC_SERVER).ok()?;
            for key in [
                &WPD_OBJECT_ORIGINAL_FILE_NAME,
                &WPD_OBJECT_NAME,
                &WPD_OBJECT_SIZE,
            ] {
                keys.Add(key as *const PROPERTYKEY).ok()?;
            }
            let wide = HSTRING::from(object);
            let values = props.GetValues(PCWSTR(wide.as_ptr()), &keys).ok()?;
            let text = |key: &PROPERTYKEY| {
                values
                    .GetStringValue(key)
                    .ok()
                    .map(|p| taken(p))
                    .filter(|s| !s.is_empty())
            };
            let name = text(&WPD_OBJECT_ORIGINAL_FILE_NAME).or_else(|| text(&WPD_OBJECT_NAME))?;
            let size = values
                .GetUnsignedLargeIntegerValue(&WPD_OBJECT_SIZE)
                .unwrap_or(0);
            Some((name, size))
        }
    }

    /// The objects inside `parent`, with their names and sizes.
    fn children(content: &IPortableDeviceContent, parent: &str) -> Vec<PhoneFile> {
        let mut out = Vec::new();
        unsafe {
            let Ok(props) = content.Properties() else {
                return out;
            };
            let wide = HSTRING::from(parent);
            let Ok(found) = content.EnumObjects(0, PCWSTR(wide.as_ptr()), None) else {
                return out;
            };
            loop {
                let mut ids = [PWSTR::null(); 32];
                let mut fetched = 0u32;
                let hr = found.Next(&mut ids, &mut fetched);
                for raw in ids.into_iter().take(fetched as usize) {
                    let object = taken(raw);
                    if let Some((name, size)) = name_of(&props, &object) {
                        out.push(PhoneFile { object, name, size });
                    }
                }
                if hr.is_err() || fetched == 0 || hr.0 != 0 {
                    break;
                }
            }
        }
        out
    }

    /// The transfer folder on `phone`, when it is there (in any of its storages); None when the phone has none, or
    /// shows no storage (it is charging only: "File transfer" is not chosen).
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
            for storage in children(&content, "DEVICE") {
                let mut at = storage.object.clone();
                let mut found = true;
                for step in FOLDER {
                    match children(&content, &at)
                        .into_iter()
                        .find(|c| c.name.eq_ignore_ascii_case(step))
                    {
                        Some(next) => at = next.object,
                        None => {
                            found = false;
                            break;
                        }
                    }
                }
                if found {
                    let files = children(&content, &at);
                    return Ok(Some(Transfer {
                        _device: device,
                        content,
                        folder: at,
                        files,
                    }));
                }
            }
            Ok(None)
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

        /// Copies `file` to `to` (a new file), telling `watch` how much has come; how many bytes. Nothing is left at
        /// `to` when it fails; one called off for sending nothing fails with [`STALLED`].
        pub fn copy(&self, file: &PhoneFile, to: &Path, watch: &Watch) -> Result<u64, String> {
            let mut out = std::fs::File::create(to).map_err(|e| e.to_string())?;
            let mut n = 0u64;
            let done = self.each_block(file, watch, &mut |block| {
                n += block.len() as u64;
                out.write_all(block).map_err(|e| e.to_string())?;
                (watch.progress)(n);
                Ok(())
            });
            if let Err(e) = done.and_then(|_| out.flush().map_err(|e| e.to_string())) {
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

        /// Deletes the transfer folder and everything in it from the phone.
        pub fn remove(&self) -> Result<(), String> {
            self.delete(&self.folder)
        }

        /// Deletes one file of the transfer from the phone (one taken: the phone counts what is left).
        pub fn remove_file(&self, file: &PhoneFile) -> Result<(), String> {
            self.delete(&file.object)
        }

        fn delete(&self, object: &str) -> Result<(), String> {
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
                    .Delete(
                        PORTABLE_DEVICE_DELETE_WITH_RECURSION.0 as u32,
                        &ids,
                        std::ptr::null_mut(),
                    )
                    .map_err(said)
            }
        }
    }
}
