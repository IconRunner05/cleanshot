use std::path::PathBuf;

const RT_MANIFEST: u16 = 24;
const PROCESS_MANIFEST_ID: u16 = 1;
const LANGUAGE_EN_US: u16 = 0x0409;
const MOVEABLE_PURE_DISCARDABLE: u16 = 0x1030;
const ENTRY_HEADER_SIZE: u32 = 32;

fn resource_entry(resource_type: u16, name: u16, language: u16, data: &[u8]) -> Vec<u8> {
    let size = u32::try_from(data.len()).expect("the manifest is smaller than 4 GB");
    let mut entry = Vec::new();
    entry.extend_from_slice(&size.to_le_bytes());
    entry.extend_from_slice(&ENTRY_HEADER_SIZE.to_le_bytes());
    entry.extend_from_slice(&[0xFF, 0xFF]);
    entry.extend_from_slice(&resource_type.to_le_bytes());
    entry.extend_from_slice(&[0xFF, 0xFF]);
    entry.extend_from_slice(&name.to_le_bytes());
    entry.extend_from_slice(&0u32.to_le_bytes());
    let flags = if data.is_empty() {
        0
    } else {
        MOVEABLE_PURE_DISCARDABLE
    };
    entry.extend_from_slice(&flags.to_le_bytes());
    entry.extend_from_slice(&language.to_le_bytes());
    entry.extend_from_slice(&0u32.to_le_bytes());
    entry.extend_from_slice(&0u32.to_le_bytes());
    entry.extend_from_slice(data);
    entry.resize(entry.len().next_multiple_of(4), 0);
    entry
}

// C-03 C-04: the .res file is written here so no resource compiler is needed on any host.
fn main() {
    let manifest_path = concat!(env!("CARGO_MANIFEST_DIR"), "/cleanshot.exe.manifest");
    println!("cargo:rerun-if-changed={manifest_path}");
    let manifest = std::fs::read(manifest_path).expect("the manifest is readable");

    let mut resources = resource_entry(0, 0, 0, &[]);
    resources.extend(resource_entry(
        RT_MANIFEST,
        PROCESS_MANIFEST_ID,
        LANGUAGE_EN_US,
        &manifest,
    ));

    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let resource_file = out_dir.join("cleanshot.res");
    std::fs::write(&resource_file, resources).expect("OUT_DIR is writable");
    println!(
        "cargo:rustc-link-arg-bin=cleanshot={}",
        resource_file.display()
    );
}
