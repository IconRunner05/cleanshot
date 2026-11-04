use std::fmt;

use windows::Win32::Foundation::{
    ERROR_BAD_LENGTH, ERROR_INSUFFICIENT_BUFFER, ERROR_NO_MORE_ITEMS, HMODULE,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_MEMORY_SEGMENT_GROUP, DXGI_MEMORY_SEGMENT_GROUP_LOCAL,
    DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO, IDXGIAdapter3,
};
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows::Win32::System::Memory::{
    GetProcessHeap, HeapLock, HeapUnlock, HeapWalk, MEM_COMMIT, MEM_IMAGE, MEM_MAPPED, MEM_PRIVATE,
    MEM_RELEASE, MEM_RESERVE, MEMORY_BASIC_INFORMATION, PAGE_READWRITE, PROCESS_HEAP_ENTRY,
    VirtualAlloc, VirtualFree, VirtualQuery,
};
use windows::Win32::System::ProcessStatus::K32QueryWorkingSet;
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows::Win32::System::SystemServices::PROCESS_HEAP_ENTRY_BUSY;
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::core::{Interface, PCWSTR};

use super::LogError;
use crate::capture::desktop_adapters;

const PAGE: usize = 4096;
/// The low bits of a working set entry, below its page's address.
const PAGE_FLAGS: usize = PAGE - 1;
/// The entry's Shared bit, set when the page is sharable.
const SHARED: usize = 1 << 8;
/// A block or allocation at least this big is listed by its size.
const LARGE: usize = 1 << 20;
/// The most large heap blocks a walk lists; the walk cannot grow a list while it holds the
/// heap's lock.
const LARGE_BLOCKS: usize = 256;

/// P-04: what holds this process's working set, by kind and size only, for `--log`; shown as
/// one line of `key=value` pairs. No module's name or path is in it (D-109).
#[derive(Debug, Default)]
pub struct Held {
    image: Pages,
    mapped: Pages,
    private: Pages,
    /// Resident pages Windows describes as none of the three.
    other: Pages,
    modules: Places,
    /// Bytes in use in the process heap, which Rust's allocator draws from.
    heap_bytes: usize,
    /// The size of each heap block in use of at least 1 MiB, largest first.
    heap_blocks: Vec<usize>,
    /// The non-sharable resident bytes of each private or mapped allocation of at least
    /// 1 MiB that holds no large heap block, largest first.
    allocations: Vec<usize>,
    /// The local and non-local video memory this process uses on each adapter that shows the
    /// desktop; one that shows none is not asked, as the capture leaves it asleep (P-01).
    video: Vec<(u64, u64)>,
}

/// Resident bytes, and the part of them that is not sharable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Pages {
    resident: usize,
    not_sharable: usize,
}

/// The image pages of the modules loaded from one [`Place`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Modules {
    count: usize,
    pages: Pages,
}

/// Image pages by where their module lives.
#[derive(Debug, Default)]
struct Places {
    ours: Modules,
    driver_store: Modules,
    system: Modules,
    other: Modules,
}

impl Places {
    fn at(&mut self, place: Place) -> &mut Modules {
        match place {
            Place::Ours => &mut self.ours,
            Place::DriverStore => &mut self.driver_store,
            Place::System => &mut self.system,
            Place::Other => &mut self.other,
        }
    }
}

/// Where a loaded module lives, which says whose it is without naming it (D-109).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    /// Cleanshot's own exe.
    Ours,
    /// The driver store, where graphics drivers' user-mode parts live.
    DriverStore,
    /// The system folder, outside the driver store.
    System,
    Other,
}

/// Where the module at `path` lives, given the system folder `system`; the driver store is
/// inside the system folder, so it is told apart first.
fn place(path: &str, system: &str) -> Place {
    let path = path.to_lowercase();
    let system = system.to_lowercase();
    let Some(inside) = path.strip_prefix(system.trim_end_matches('\\')) else {
        return Place::Other;
    };
    if inside.starts_with("\\driverstore\\") {
        Place::DriverStore
    } else if inside.starts_with('\\') {
        Place::System
    } else {
        Place::Other
    }
}

/// What holds this process's working set now. Its one large buffer, the page list, comes from
/// `VirtualAlloc` and goes back with `VirtualFree`, so the heap keeps none of it.
///
/// # Errors
///
/// [`LogError::Memory`] if Windows does not list the working set, walk the heap, give the
/// system folder or list the adapters.
pub fn held() -> Result<Held, LogError> {
    let heap = heap()?;
    let mut held = pages(&heap)?;
    held.heap_bytes = heap.busy;
    held.heap_blocks = heap.large.iter().map(|&(_, size)| size).collect();
    held.heap_blocks.sort_unstable_by(|a, b| b.cmp(a));
    held.video = video()?;
    Ok(held)
}

struct Heap {
    busy: usize,
    /// The address and size of each large block in use.
    large: Vec<(usize, usize)>,
}

fn heap() -> Result<Heap, LogError> {
    // SAFETY: GetProcessHeap has no preconditions.
    let process = unsafe { GetProcessHeap() }.map_err(LogError::Memory)?;
    let mut large = Vec::with_capacity(LARGE_BLOCKS);
    let mut busy = 0;
    let mut entry = PROCESS_HEAP_ENTRY::default();
    // SAFETY: the heap is locked for the walk, since HeapWalk can fail when other threads use
    // an unlocked heap, and unlocked after; `entry` starts zeroed (lpData null) and is left
    // unchanged between calls, and nothing allocates while it is locked: `large` only grows
    // within its capacity.
    let walked = unsafe {
        HeapLock(process).map_err(LogError::Memory)?;
        let walked = loop {
            match HeapWalk(process, &raw mut entry) {
                Ok(()) => {}
                Err(error) if error.code() == ERROR_NO_MORE_ITEMS.to_hresult() => break Ok(()),
                Err(error) => break Err(error),
            }
            if u32::from(entry.wFlags) & PROCESS_HEAP_ENTRY_BUSY == 0 {
                continue;
            }
            let size = entry.cbData as usize;
            busy += size;
            if size >= LARGE && large.len() < large.capacity() {
                large.push((entry.lpData as usize, size));
            }
        };
        HeapUnlock(process).map_err(LogError::Memory)?;
        walked
    };
    walked.map_err(LogError::Memory)?;
    Ok(Heap { busy, large })
}

fn pages(heap: &Heap) -> Result<Held, LogError> {
    let mut list = working_set()?;
    let entries = list.entries();
    entries.sort_unstable();
    let system = system_folder()?;
    // SAFETY: a lookup of this process's own exe, which is never unloaded.
    let exe = unsafe { GetModuleHandleW(PCWSTR::null()) }.map_err(LogError::Memory)?;
    let mut held = Held::default();
    let mut region = MEMORY_BASIC_INFORMATION::default();
    let mut allocations: Vec<Allocation> = Vec::new();
    // The pages are in address order, so a module's pages come together.
    let mut module: Option<(usize, Place)> = None;
    for &entry in &*entries {
        let page = entry & !PAGE_FLAGS;
        let not_sharable = if entry & SHARED == 0 { PAGE } else { 0 };
        let start = region.BaseAddress as usize;
        if !(start..start + region.RegionSize).contains(&page) {
            // SAFETY: `region` is writable and its size is passed; any address may be asked.
            let written = unsafe {
                VirtualQuery(
                    Some(page as *const _),
                    &raw mut region,
                    size_of::<MEMORY_BASIC_INFORMATION>(),
                )
            };
            if written == 0 {
                region = MEMORY_BASIC_INFORMATION::default();
                add(&mut held.other, not_sharable);
                continue;
            }
        }
        let base = region.AllocationBase as usize;
        let end = region.BaseAddress as usize + region.RegionSize;
        let kind = region.Type;
        if kind == MEM_IMAGE {
            add(&mut held.image, not_sharable);
            let place = match module {
                Some((placed, place)) if placed == base => place,
                _ => {
                    let place = module_place(base, exe, &system);
                    held.modules.at(place).count += 1;
                    module = Some((base, place));
                    place
                }
            };
            add(&mut held.modules.at(place).pages, not_sharable);
        } else if kind == MEM_MAPPED || kind == MEM_PRIVATE {
            add(
                if kind == MEM_MAPPED {
                    &mut held.mapped
                } else {
                    &mut held.private
                },
                not_sharable,
            );
            match allocations.last_mut() {
                Some(allocation) if allocation.base == base => {
                    allocation.end = end;
                    allocation.not_sharable += not_sharable;
                }
                _ => allocations.push(Allocation {
                    base,
                    end,
                    not_sharable,
                }),
            }
        } else {
            add(&mut held.other, not_sharable);
        }
    }
    held.allocations = allocations
        .into_iter()
        .filter(|allocation| {
            allocation.not_sharable >= LARGE
                && !heap
                    .large
                    .iter()
                    .any(|&(block, _)| (allocation.base..allocation.end).contains(&block))
        })
        .map(|allocation| allocation.not_sharable)
        .collect();
    held.allocations.sort_unstable_by(|a, b| b.cmp(a));
    Ok(held)
}

/// One allocation's address range, as far as its resident pages reach, and its non-sharable
/// resident bytes.
struct Allocation {
    base: usize,
    end: usize,
    not_sharable: usize,
}

fn add(pages: &mut Pages, not_sharable: usize) {
    pages.resident += PAGE;
    pages.not_sharable += not_sharable;
}

/// A `PSAPI_WORKING_SET_INFORMATION` in pages of its own: the entry count, then one
/// pointer-sized entry per page, its address in the high bits and its flags in the low 12.
struct PageList {
    start: *mut usize,
    /// How many pointer-sized words fit, the count included.
    words: usize,
}

impl PageList {
    fn new(words: usize) -> Result<Self, LogError> {
        // SAFETY: a fresh read-write allocation anywhere; null means it failed.
        let start = unsafe {
            VirtualAlloc(
                None,
                words * size_of::<usize>(),
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READWRITE,
            )
        };
        if start.is_null() {
            return Err(LogError::Memory(windows::core::Error::from_thread()));
        }
        Ok(Self {
            start: start.cast(),
            words,
        })
    }

    fn count(&self) -> usize {
        // SAFETY: the first word is committed and was zeroed by VirtualAlloc or written by
        // QueryWorkingSet.
        unsafe { *self.start }
    }

    /// The listed entries, once `QueryWorkingSet` has filled the list.
    fn entries(&mut self) -> &mut [usize] {
        let count = self.count().min(self.words - 1);
        // SAFETY: the words after the count are committed, owned by this list, and the first
        // `count` of them were written by QueryWorkingSet.
        unsafe { std::slice::from_raw_parts_mut(self.start.add(1), count) }
    }
}

impl Drop for PageList {
    fn drop(&mut self) {
        // SAFETY: `start` came from VirtualAlloc and is released once, here.
        let freed = unsafe { VirtualFree(self.start.cast(), 0, MEM_RELEASE) };
        debug_assert!(freed.is_ok(), "a list this process allocated is freed");
    }
}

fn working_set() -> Result<PageList, LogError> {
    let mut list = PageList::new(PAGE / size_of::<usize>())?;
    loop {
        let bytes = u32::try_from(list.words * size_of::<usize>())
            .expect("a working set list fits in 4 GB");
        // SAFETY: the list is writable for `bytes`, laid out as PSAPI_WORKING_SET_INFORMATION.
        let listed = unsafe { K32QueryWorkingSet(GetCurrentProcess(), list.start.cast(), bytes) };
        if listed.as_bool() {
            return Ok(list);
        }
        let error = windows::core::Error::from_thread();
        if error.code() != ERROR_BAD_LENGTH.to_hresult() {
            return Err(LogError::Memory(error));
        }
        // The count now says how many entries are needed. The working set grows while the
        // next list is made, so room is left for the pages it adds.
        let needed = list.count();
        list = PageList::new(1 + needed + needed / 8 + 1024)?;
    }
}

fn module_place(base: usize, exe: HMODULE, system: &str) -> Place {
    let module = HMODULE(base as *mut _);
    if module == exe {
        return Place::Ours;
    }
    let mut path = [0u16; 1024];
    // SAFETY: `path` is writable.
    let length = unsafe { GetModuleFileNameW(Some(module), &mut path) } as usize;
    // A base that is no loaded module's, such as an image mapped as data, gives 0, which the
    // docs do not tell apart from a failure, and a path the buffer cuts short gives its
    // length; neither can be placed, so both count as Other.
    if length == 0 || length == path.len() {
        return Place::Other;
    }
    place(&String::from_utf16_lossy(&path[..length]), system)
}

fn system_folder() -> Result<String, LogError> {
    let mut folder = [0u16; 512];
    // SAFETY: `folder` is writable.
    let length = unsafe { GetSystemDirectoryW(Some(&mut folder)) } as usize;
    if length == 0 {
        return Err(LogError::Memory(windows::core::Error::from_thread()));
    }
    // A folder too long for the buffer gives the size it needs instead.
    let folder = folder
        .get(..length)
        .ok_or_else(|| LogError::Memory(ERROR_INSUFFICIENT_BUFFER.into()))?;
    Ok(String::from_utf16_lossy(folder))
}

fn video() -> Result<Vec<(u64, u64)>, LogError> {
    let mut used = Vec::new();
    for (_, adapter) in desktop_adapters().map_err(LogError::Memory)? {
        let adapter: IDXGIAdapter3 = adapter.cast().map_err(LogError::Memory)?;
        used.push((
            usage(&adapter, DXGI_MEMORY_SEGMENT_GROUP_LOCAL)?,
            usage(&adapter, DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL)?,
        ));
    }
    Ok(used)
}

fn usage(adapter: &IDXGIAdapter3, group: DXGI_MEMORY_SEGMENT_GROUP) -> Result<u64, LogError> {
    let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
    // SAFETY: `info` is writable; node 0 is the adapter itself.
    unsafe { adapter.QueryVideoMemoryInfo(0, group, &raw mut info) }.map_err(LogError::Memory)?;
    Ok(info.CurrentUsage)
}

/// Not sharable, then resident, in bytes.
impl fmt::Display for Pages {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.not_sharable, self.resident)
    }
}

/// The count, then the pages.
impl fmt::Display for Modules {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.count, self.pages)
    }
}

impl fmt::Display for Held {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Places {
            ours,
            driver_store,
            system,
            other,
        } = &self.modules;
        write!(
            formatter,
            "image={} mapped={} private={} other={} modules_ours={ours} \
             modules_driver_store={driver_store} modules_system={system} modules_other={other} \
             heap_bytes={} heap_blocks={:?} allocations={:?} video={:?}",
            self.image,
            self.mapped,
            self.private,
            self.other,
            self.heap_bytes,
            self.heap_blocks,
            self.allocations,
            self.video
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p04_a_module_is_placed_by_its_folder_with_the_driver_store_apart_from_the_system() {
        let system = "C:\\Windows\\system32";
        let placed = [
            "C:\\WINDOWS\\System32\\DriverStore\\FileRepository\\iigd_dch.inf_amd64\\igd10iumd64.dll",
            "C:\\Windows\\System32\\d3d11.dll",
            "C:\\Windows\\System32Extra\\x.dll",
            "C:\\Program Files\\Agent\\hook.dll",
        ]
        .map(|path| place(path, system));
        assert_eq!(
            placed,
            [
                Place::DriverStore,
                Place::System,
                Place::Other,
                Place::Other
            ]
        );
    }

    #[test]
    fn p04_the_held_pages_add_up_to_the_private_working_set_windows_counts() {
        let counted = super::super::usage().expect("the usage reads");
        let held = held().expect("the working set is listed");
        let not_sharable = held.image.not_sharable
            + held.mapped.not_sharable
            + held.private.not_sharable
            + held.other.not_sharable;
        let (low, high) = (
            counted.private_working_set_bytes / 2,
            counted.private_working_set_bytes * 2,
        );
        assert!(
            (low..high).contains(&not_sharable),
            "listed {not_sharable} non-sharable bytes against Windows' private {}",
            counted.private_working_set_bytes
        );
        assert!(held.heap_bytes > 0, "a running process uses its heap");
    }

    #[test]
    fn p04_a_large_heap_block_is_listed_by_its_size() {
        let block = vec![1u8; 3 * LARGE];
        let held = held().expect("the working set is listed");
        assert!(
            held.heap_blocks.contains(&block.len()),
            "{:?} lists no block of {}",
            held.heap_blocks,
            block.len()
        );
        drop(std::hint::black_box(block));
    }
}
