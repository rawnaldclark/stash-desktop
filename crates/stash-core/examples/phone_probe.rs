// Lists the phones Windows sees and their Stash transfer folders. With a phone's name and a folder, it also copies
// the first files of that phone's transfer folder there, as the core takes them (each read watched for stalls), and
// says how fast each came: nothing on the phone is changed or deleted, and nothing goes into a library.
//
//     cargo run -p stash-core --example phone_probe -- "Pixel 5" C:\scratch\probe 20
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let phones = stash_core::phone::phones();
    println!(
        "phones: {:?}",
        phones
            .as_ref()
            .map(|p| p.iter().map(|p| p.name.clone()).collect::<Vec<_>>())
    );
    #[cfg(windows)]
    for p in phones.unwrap_or_default() {
        match stash_core::phone::open_transfer(&p.id) {
            Ok(Some(t)) => {
                let total: u64 = t.files.iter().map(|f| f.size).sum();
                println!(
                    "{}: transfer folder with {} files, {} MB; list there: {}",
                    p.name,
                    t.files.len(),
                    total / 1_000_000,
                    t.files.iter().any(|f| f.name == stash_core::phone::LIST)
                );
                if let Some(l) = t.files.iter().find(|f| f.name == stash_core::phone::LIST) {
                    println!("  list read: {:?}", t.bytes(l, 16 << 20).map(|b| b.len()));
                }
                let (Some(name), Some(dir)) = (args.first(), args.get(1)) else {
                    continue;
                };
                if &p.name != name {
                    continue;
                }
                let dir = std::path::PathBuf::from(dir);
                std::fs::create_dir_all(&dir).expect("the folder to copy into");
                let n: usize = args.get(2).and_then(|n| n.parse().ok()).unwrap_or(10);
                let (mut bytes, started) = (0u64, std::time::Instant::now());
                for f in t.files.iter().filter(|f| f.name != stash_core::phone::LIST).take(n) {
                    let at = std::time::Instant::now();
                    let stalled = || println!("  {}: sent nothing for a while: calling the read off", f.name);
                    let watch = stash_core::phone::Watch {
                        stall_after: stash_core::phone::STALL,
                        progress: &|_| {},
                        stalled: &stalled,
                    };
                    let to = dir.join(&f.name);
                    match t.copy(f, &to, &watch) {
                        Ok(got) => {
                            bytes += got;
                            let secs = at.elapsed().as_secs_f64();
                            println!("  {} {} MB in {secs:.1} s ({:.1} MB/s)", f.name, got / 1_000_000, got as f64 / 1e6 / secs.max(0.001));
                        }
                        Err(e) => println!("  {}: {e} after {:.1} s", f.name, at.elapsed().as_secs_f64()),
                    }
                    let _ = std::fs::remove_file(&to);
                }
                let secs = started.elapsed().as_secs_f64();
                println!("  all: {} MB in {secs:.1} s ({:.1} MB/s)", bytes / 1_000_000, bytes as f64 / 1e6 / secs.max(0.001));
            }
            Ok(None) => println!("{}: no transfer folder (or no storage shown)", p.name),
            Err(e) => println!("{}: {e}", p.name),
        }
    }
}
