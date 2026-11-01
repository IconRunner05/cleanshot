use std::path::PathBuf;

const RT_VERSION: u16 = 16;
const RT_MANIFEST: u16 = 24;
const PROCESS_MANIFEST_ID: u16 = 1;
const VERSION_ID: u16 = 1;
const LANGUAGE_EN_US: u16 = 0x0409;
const UNICODE_CODE_PAGE: u16 = 1200;
const MOVEABLE_PURE_DISCARDABLE: u16 = 0x1030;
const ENTRY_HEADER_SIZE: u32 = 32;
const FIXED_SIGNATURE: u32 = 0xFEEF_04BD;
const FIXED_LAYOUT_1_0: u32 = 0x0001_0000;
const ALL_FILE_FLAGS: u32 = 0x3F;
const VOS_NT_WINDOWS32: u32 = 0x0004_0004;
const VFT_APP: u32 = 1;

fn resource_entry(resource_type: u16, name: u16, language: u16, data: &[u8]) -> Vec<u8> {
    let size = u32::try_from(data.len()).expect("a resource is smaller than 4 GB");
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

fn utf16_with_nul(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// One node of a VERSIONINFO tree: its length, value length and type, then its key, value
/// and children, the last three each starting on a 4-byte boundary.
fn version_node(
    key: &str,
    is_text: bool,
    value_length: u16,
    value: &[u8],
    children: &[Vec<u8>],
) -> Vec<u8> {
    let mut node = vec![0; 6];
    node.extend(utf16_with_nul(key));
    node.resize(node.len().next_multiple_of(4), 0);
    node.extend_from_slice(value);
    for child in children {
        node.resize(node.len().next_multiple_of(4), 0);
        node.extend_from_slice(child);
    }
    let length = u16::try_from(node.len()).expect("a version node is under 64 KB");
    node[0..2].copy_from_slice(&length.to_le_bytes());
    node[2..4].copy_from_slice(&value_length.to_le_bytes());
    node[4..6].copy_from_slice(&u16::from(is_text).to_le_bytes());
    node
}

fn version_string(key: &str, value: &str) -> Vec<u8> {
    let characters =
        u16::try_from(value.encode_utf16().count() + 1).expect("a version string is short");
    version_node(key, true, characters, &utf16_with_nul(value), &[])
}

fn version_part(name: &str) -> u32 {
    std::env::var(name)
        .expect("cargo sets the version parts")
        .parse()
        .expect("a version part is a number")
}

/// The product name and version the code-signing terms (D-74) require in a signed exe, which
/// Explorer's Properties shows too.
fn version_info() -> Vec<u8> {
    let most =
        (version_part("CARGO_PKG_VERSION_MAJOR") << 16) | version_part("CARGO_PKG_VERSION_MINOR");
    let least = version_part("CARGO_PKG_VERSION_PATCH") << 16;
    let fixed: Vec<u8> = [
        FIXED_SIGNATURE,
        FIXED_LAYOUT_1_0,
        most,
        least,
        most,
        least,
        ALL_FILE_FLAGS,
        0,
        VOS_NT_WINDOWS32,
        VFT_APP,
        0,
        0,
        0,
    ]
    .into_iter()
    .flat_map(u32::to_le_bytes)
    .collect();
    let version = std::env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION");
    let strings = version_node(
        "040904B0",
        true,
        0,
        &[],
        &[
            version_string("ProductName", "Cleanshot"),
            version_string("FileDescription", "Cleanshot"),
            version_string("ProductVersion", &version),
            version_string("FileVersion", &version),
        ],
    );
    let translation: Vec<u8> = [LANGUAGE_EN_US, UNICODE_CODE_PAGE]
        .into_iter()
        .flat_map(u16::to_le_bytes)
        .collect();
    let fixed_length = u16::try_from(fixed.len()).expect("the fixed block is 52 bytes");
    version_node(
        "VS_VERSION_INFO",
        false,
        fixed_length,
        &fixed,
        &[
            version_node("StringFileInfo", true, 0, &[], &[strings]),
            version_node(
                "VarFileInfo",
                true,
                0,
                &[],
                &[version_node("Translation", false, 4, &translation, &[])],
            ),
        ],
    )
}

// C-03 C-04: the .res file is written here so no resource compiler is needed on any host.
fn main() {
    // The field-test probe builds with this script too, so the manifest is found next to the
    // script rather than in the package being built.
    let manifest = include_bytes!("cleanshot.exe.manifest");

    let mut resources = resource_entry(0, 0, 0, &[]);
    resources.extend(resource_entry(
        RT_MANIFEST,
        PROCESS_MANIFEST_ID,
        LANGUAGE_EN_US,
        manifest,
    ));
    resources.extend(resource_entry(
        RT_VERSION,
        VERSION_ID,
        LANGUAGE_EN_US,
        &version_info(),
    ));

    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let resource_file = out_dir.join("cleanshot.res");
    std::fs::write(&resource_file, resources).expect("OUT_DIR is writable");
    println!("cargo:rustc-link-arg-bins={}", resource_file.display());
    // GPUI imports comctl32 v6 functions, so a test that opens the editor needs the manifest
    // as much as the exe does. The field-test probe has no test target.
    if std::env::var("CARGO_PKG_NAME").as_deref() == Ok("cleanshot-app") {
        println!("cargo:rustc-link-arg-tests={}", resource_file.display());
    }
}
