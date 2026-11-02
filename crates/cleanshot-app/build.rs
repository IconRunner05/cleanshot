use std::path::PathBuf;

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const RT_VERSION: u16 = 16;
const RT_MANIFEST: u16 = 24;
const PROCESS_MANIFEST_ID: u16 = 1;
const VERSION_ID: u16 = 1;
const LANGUAGE_EN_US: u16 = 0x0409;
const LANGUAGE_NEUTRAL: u16 = 0;
const ICON_FILE_HEADER_SIZE: usize = 6;
const ICON_FILE_ENTRY_SIZE: usize = 16;
/// D-91: Explorer, the taskbar and GPUI's windows show icon group 1.
const APP_ICON_GROUP: u16 = 1;
const APP_ICON: &[u8] = include_bytes!("../../assets/cleanshot.ico");
const UNICODE_CODE_PAGE: u16 = 1200;
const MOVEABLE_PURE_DISCARDABLE: u16 = 0x1030;
const ENTRY_HEADER_SIZE: u32 = 32;
const FIXED_SIGNATURE: u32 = 0xFEEF_04BD;
const FIXED_LAYOUT_1_0: u32 = 0x0001_0000;
const ALL_FILE_FLAGS: u32 = 0x3F;
const VOS_NT_WINDOWS32: u32 = 0x0004_0004;
const VFT_APP: u32 = 1;
const DELAY_LOADED: [&str; 11] = [
    "bcryptprimitives.dll",
    "comctl32.dll",
    "d2d1.dll",
    "d3d11.dll",
    "dcomp.dll",
    "dwmapi.dll",
    "dwrite.dll",
    "dxgi.dll",
    "icuuc.dll",
    "uiautomationcore.dll",
    "winmm.dll",
];

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

/// Each image of [`APP_ICON`] as an `RT_ICON` numbered from 1, and its directory as the
/// `RT_GROUP_ICON` naming them by those numbers.
fn icon_resources() -> Vec<u8> {
    let count = usize::from(u16::from_le_bytes([APP_ICON[4], APP_ICON[5]]));
    let (entries, _) = APP_ICON[ICON_FILE_HEADER_SIZE..].as_chunks::<ICON_FILE_ENTRY_SIZE>();
    let mut resources = Vec::new();
    let mut directory = APP_ICON[..ICON_FILE_HEADER_SIZE].to_vec();
    for (image_id, entry) in (1u16..).zip(&entries[..count]) {
        let read_u32 = |at: usize| {
            let bytes = entry[at..at + 4]
                .try_into()
                .expect("an .ico entry is 16 bytes");
            usize::try_from(u32::from_le_bytes(bytes)).expect("an .ico fits in memory")
        };
        let (length, offset) = (read_u32(8), read_u32(12));
        resources.extend(resource_entry(
            RT_ICON,
            image_id,
            LANGUAGE_NEUTRAL,
            &APP_ICON[offset..offset + length],
        ));
        // The group entry is the file entry with one plane and the image's id in place of its
        // offset; the file says zero planes.
        directory.extend_from_slice(&entry[..4]);
        directory.extend_from_slice(&1u16.to_le_bytes());
        directory.extend_from_slice(&entry[6..12]);
        directory.extend_from_slice(&image_id.to_le_bytes());
    }
    resources.extend(resource_entry(
        RT_GROUP_ICON,
        APP_ICON_GROUP,
        LANGUAGE_NEUTRAL,
        &directory,
    ));
    resources
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
    resources.extend(icon_resources());

    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let resource_file = out_dir.join("cleanshot.res");
    std::fs::write(&resource_file, resources).expect("OUT_DIR is writable");
    println!("cargo:rustc-link-arg-bins={}", resource_file.display());
    // GPUI imports comctl32 v6 functions, so a test that opens the editor needs the manifest
    // as much as the exe does. The field-test probe has no test target.
    if std::env::var("CARGO_PKG_NAME").as_deref() == Ok("cleanshot-app") {
        println!("cargo:rustc-link-arg-tests={}", resource_file.display());
        // C-12: a DLL outside KnownDLLs loads on first use, after main limits search to System32.
        for dll in DELAY_LOADED {
            println!("cargo:rustc-link-arg-bins=/DELAYLOAD:{dll}");
        }
        println!("cargo:rustc-link-arg-bins=delayimp.lib");
    }
}
