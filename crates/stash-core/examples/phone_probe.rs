// Not committed: lists the phones Windows sees and their Stash transfer folders.
fn main() {
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
                for f in t.files.iter().take(3) {
                    println!("  {} {} bytes: head {:?}", f.name, f.size, t.head(f, 4));
                }
                if let Some(l) = t.files.iter().find(|f| f.name == stash_core::phone::LIST) {
                    println!("  list read: {:?}", t.bytes(l, 16 << 20).map(|b| b.len()));
                }
            }
            Ok(None) => println!("{}: no transfer folder (or no storage shown)", p.name),
            Err(e) => println!("{}: {e}", p.name),
        }
    }
}
