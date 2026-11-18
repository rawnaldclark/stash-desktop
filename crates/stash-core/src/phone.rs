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

    use windows::Win32::Devices::PortableDevices::*;
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
        CoTaskMemFree, IStream, STGM_READ,
    };
    use windows::Win32::System::Variant::VT_LPWSTR;
    use windows::core::{HSTRING, PCWSTR, PWSTR};

    use super::{FOLDER, Phone, PhoneFile};

    fn com() {
        // Already initialised on this thread (in either mode) is fine.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    }

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
            self.each_block(file, &mut |block| {
                if out.len() as u64 + block.len() as u64 > cap {
                    return Err("the list is too big".into());
                }
                out.extend_from_slice(block);
                Ok(())
            })?;
            Ok(out)
        }

        /// The first `n` bytes of `file` (fewer when it is shorter): what kind of file it is.
        pub fn head(&self, file: &PhoneFile, n: usize) -> Result<Vec<u8>, String> {
            const ENOUGH: &str = " enough";
            let mut out = Vec::new();
            let read = self.each_block(file, &mut |block| {
                out.extend_from_slice(&block[..block.len().min(n - out.len())]);
                if out.len() >= n {
                    Err(ENOUGH.into())
                } else {
                    Ok(())
                }
            });
            match read {
                Ok(()) => Ok(out),
                Err(e) if e == ENOUGH => Ok(out),
                Err(e) => Err(e),
            }
        }

        /// Copies `file` to `to` (a new file); how many bytes. Nothing is left at `to` when it fails.
        pub fn copy(&self, file: &PhoneFile, to: &Path) -> Result<u64, String> {
            let mut out = std::fs::File::create(to).map_err(|e| e.to_string())?;
            let mut n = 0u64;
            let done = self.each_block(file, &mut |block| {
                n += block.len() as u64;
                out.write_all(block).map_err(|e| e.to_string())
            });
            if let Err(e) = done.and_then(|_| out.flush().map_err(|e| e.to_string())) {
                drop(out);
                let _ = std::fs::remove_file(to);
                return Err(e);
            }
            Ok(n)
        }

        fn each_block(
            &self,
            file: &PhoneFile,
            take: &mut dyn FnMut(&[u8]) -> Result<(), String>,
        ) -> Result<(), String> {
            unsafe {
                let resources = self.content.Transfer().map_err(said)?;
                let wide = HSTRING::from(file.object.as_str());
                let mut optimal = 0u32;
                let mut stream: Option<IStream> = None;
                resources
                    .GetStream(
                        PCWSTR(wide.as_ptr()),
                        &WPD_RESOURCE_DEFAULT,
                        STGM_READ.0,
                        &mut optimal,
                        &mut stream,
                    )
                    .map_err(said)?;
                let stream = stream.ok_or_else(|| "the phone gave no file".to_string())?;
                let mut buf = vec![0u8; (optimal as usize).clamp(64 * 1024, 4 * 1024 * 1024)];
                loop {
                    let mut read = 0u32;
                    let hr = stream.Read(
                        buf.as_mut_ptr() as *mut _,
                        buf.len() as u32,
                        Some(&mut read),
                    );
                    if hr.is_err() {
                        return Err(format!("the phone stopped sending ({:#x})", hr.0));
                    }
                    if read == 0 {
                        return Ok(());
                    }
                    take(&buf[..read as usize])?;
                }
            }
        }

        /// Deletes the transfer folder and everything in it from the phone.
        pub fn remove(&self) -> Result<(), String> {
            unsafe {
                let ids: IPortableDevicePropVariantCollection = CoCreateInstance(
                    &PortableDevicePropVariantCollection,
                    None,
                    CLSCTX_INPROC_SERVER,
                )
                .map_err(said)?;
                let wide = HSTRING::from(self.folder.as_str());
                // One VT_LPWSTR naming the folder. `Add` keeps its own copy, so this one is never cleared (the string
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
