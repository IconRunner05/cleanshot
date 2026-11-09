use std::cell::RefCell;
use std::mem::ManuallyDrop;
use std::rc::Rc;

use windows::Win32::Foundation::{
    DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, DV_E_FORMATETC, E_INVALIDARG,
    E_NOTIMPL, OLE_E_ADVISENOTSUPPORTED, OLE_E_NOTRUNNING, S_OK,
};
use windows::Win32::System::Com::{
    DATADIR_GET, DVASPECT_CONTENT, FORMATETC, IAdviseSink, IDataObject, IDataObject_Impl,
    IEnumFORMATETC, IEnumSTATDATA, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL, TYMED_ISTREAM,
};
use windows::Win32::System::Ole::{
    CF_DIBV5, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE, DoDragDrop, IDropSource,
    IDropSource_Impl,
};
use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Shell::{
    CFSTR_FILECONTENTS, CFSTR_FILEDESCRIPTORW, FD_FILESIZE, FD_PROGRESSUI, FILEDESCRIPTORW,
    FILEGROUPDESCRIPTORW, SHCreateMemStream, SHCreateStdEnumFmtEtc,
};
use windows::core::{BOOL, HRESULT, Ref, implement};

use crate::clipboard::{global, png_format, registered};

/// What the thumbnail offers when it is dragged out: a file named `name` holding
/// `png`, made in memory and never written to disk, and the same image as PNG and as
/// `bitmap`, a `CF_DIBV5` block, for apps that take a picture rather than a file.
pub struct Dragged {
    pub name: String,
    pub png: Vec<u8>,
    pub bitmap: Vec<u8>,
}

/// What a drag ended in.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Ended {
    Dropped,
    Cancelled,
}

/// Runs Windows' drag loop with `dragged` until it is dropped or cancelled. The thread must
/// have OLE started, as GPUI's does.
pub(super) fn drag(dragged: Dragged) -> windows::core::Result<Ended> {
    let ended = offered_during(dragged, |data| {
        let source: IDropSource = Source.into();
        let mut effect = DROPEFFECT_NONE;
        // SAFETY: both objects live for the call, and `effect` is writable.
        unsafe { DoDragDrop(data, &source, DROPEFFECT_COPY, &raw mut effect) }
    })?;
    if ended == DRAGDROP_S_DROP {
        return Ok(Ended::Dropped);
    }
    if ended == DRAGDROP_S_CANCEL {
        return Ok(Ended::Cancelled);
    }
    Err(ended.into())
}

/// Offers `dragged` for as long as `during` runs and then frees it: a drop target can
/// keep the object it was handed, and what it reads from it after that is an error.
fn offered_during<T>(
    dragged: Dragged,
    during: impl FnOnce(&IDataObject) -> T,
) -> windows::core::Result<T> {
    let offer = Offer::new(dragged)?;
    let image = offer.image();
    let data: IDataObject = offer.into();
    let result = during(&data);
    drop(image.take());
    Ok(result)
}

/// One kind of data the drag offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Offered {
    FileDescriptor,
    FileContents,
    Png,
    Bitmap,
}

const OFFERED: [Offered; 4] = [
    Offered::FileDescriptor,
    Offered::FileContents,
    Offered::Png,
    Offered::Bitmap,
];

fn format(offered: Offered) -> windows::core::Result<FORMATETC> {
    let (format, tymed, index) = match offered {
        Offered::FileDescriptor => (registered(CFSTR_FILEDESCRIPTORW)?, TYMED_HGLOBAL, -1),
        Offered::FileContents => (registered(CFSTR_FILECONTENTS)?, TYMED_ISTREAM, 0),
        Offered::Png => (png_format()?, TYMED_HGLOBAL, -1),
        Offered::Bitmap => (u32::from(CF_DIBV5.0), TYMED_HGLOBAL, -1),
    };
    Ok(FORMATETC {
        cfFormat: u16::try_from(format).map_err(|_| windows::core::Error::from(E_INVALIDARG))?,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: index,
        tymed: u32::try_from(tymed.0).expect("a TYMED flag is positive"),
    })
}

#[expect(
    clippy::inline_always,
    clippy::ref_as_ptr,
    reason = "in the code #[implement] generates"
)]
mod objects {
    use super::{
        BOOL, DATADIR_GET, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS,
        DROPEFFECT, DV_E_FORMATETC, Dragged, E_INVALIDARG, E_NOTIMPL, FD_FILESIZE, FD_PROGRESSUI,
        FILEDESCRIPTORW, FILEGROUPDESCRIPTORW, FORMATETC, HRESULT, IAdviseSink, IDataObject,
        IDataObject_Impl, IDropSource, IDropSource_Impl, IEnumFORMATETC, IEnumSTATDATA, MK_LBUTTON,
        MODIFIERKEYS_FLAGS, ManuallyDrop, OFFERED, OLE_E_ADVISENOTSUPPORTED, OLE_E_NOTRUNNING,
        Offered, Rc, Ref, RefCell, S_OK, SHCreateMemStream, SHCreateStdEnumFmtEtc, STGMEDIUM,
        STGMEDIUM_0, TYMED_HGLOBAL, TYMED_ISTREAM, format, global, implement,
    };

    /// The data the drag carries, made when the drag starts. Its image goes when the drag ends.
    #[implement(IDataObject)]
    pub(super) struct Offer {
        descriptor: Vec<u8>,
        image: Rc<RefCell<Option<Dragged>>>,
        formats: [(Offered, FORMATETC); 4],
    }

    impl Offer {
        pub(super) fn new(dragged: Dragged) -> windows::core::Result<Self> {
            let invalid = || windows::core::Error::from(E_INVALIDARG);
            let units: Vec<u16> = dragged.name.encode_utf16().collect();
            let mut name = [0_u16; 260];
            name.get_mut(..units.len())
                .filter(|_| units.len() < 260)
                .ok_or_else(invalid)?
                .copy_from_slice(&units);
            let group = FILEGROUPDESCRIPTORW {
                cItems: 1,
                fgd: [FILEDESCRIPTORW {
                    dwFlags: u32::try_from(FD_FILESIZE.0 | FD_PROGRESSUI.0)
                        .map_err(|_| invalid())?,
                    nFileSizeLow: u32::try_from(dragged.png.len()).map_err(|_| invalid())?,
                    cFileName: name,
                    ..Default::default()
                }],
            };
            // SAFETY: `group` is a plain packed struct, read as its own bytes while it lives.
            let descriptor = unsafe {
                std::slice::from_raw_parts(
                    (&raw const group).cast::<u8>(),
                    size_of::<FILEGROUPDESCRIPTORW>(),
                )
            }
            .to_vec();
            let formats = OFFERED.map(|offered| format(offered).map(|format| (offered, format)));
            let [a, b, c, d] = formats;
            Ok(Self {
                descriptor,
                image: Rc::new(RefCell::new(Some(dragged))),
                formats: [a?, b?, c?, d?],
            })
        }

        pub(super) fn image(&self) -> Rc<RefCell<Option<Dragged>>> {
            Rc::clone(&self.image)
        }

        /// What `asked` names, if the drag offers it in a medium the asker takes.
        fn offered(&self, asked: &FORMATETC) -> Option<Offered> {
            self.formats.iter().find_map(|&(offered, ours)| {
                let matches = asked.cfFormat == ours.cfFormat
                    && asked.dwAspect == ours.dwAspect
                    && asked.tymed & ours.tymed != 0
                    && (asked.lindex == -1 || asked.lindex == ours.lindex.max(0));
                matches.then_some(offered)
            })
        }
    }

    #[allow(non_snake_case)]
    impl IDataObject_Impl for Offer_Impl {
        fn GetData(&self, asked: *const FORMATETC) -> windows::core::Result<STGMEDIUM> {
            // SAFETY: OLE passes a valid FORMATETC for the call.
            let asked = unsafe { asked.as_ref() }.ok_or(E_INVALIDARG)?;
            let hglobal = |bytes: &[u8]| -> windows::core::Result<STGMEDIUM> {
                Ok(STGMEDIUM {
                    tymed: u32::try_from(TYMED_HGLOBAL.0).expect("positive"),
                    u: STGMEDIUM_0 {
                        hGlobal: global(bytes)?,
                    },
                    pUnkForRelease: ManuallyDrop::new(None),
                })
            };
            let offered = self.offered(asked).ok_or(DV_E_FORMATETC)?;
            let image = self.image.borrow();
            let dragged = image.as_ref().ok_or(OLE_E_NOTRUNNING)?;
            match offered {
                Offered::FileDescriptor => hglobal(&self.descriptor),
                Offered::Png => hglobal(&dragged.png),
                Offered::Bitmap => hglobal(&dragged.bitmap),
                Offered::FileContents => {
                    // SAFETY: the stream copies the bytes it is given.
                    let stream = unsafe { SHCreateMemStream(Some(&dragged.png)) }
                        .ok_or_else(windows::core::Error::from_thread)?;
                    Ok(STGMEDIUM {
                        tymed: u32::try_from(TYMED_ISTREAM.0).expect("positive"),
                        u: STGMEDIUM_0 {
                            pstm: ManuallyDrop::new(Some(stream)),
                        },
                        pUnkForRelease: ManuallyDrop::new(None),
                    })
                }
            }
        }

        fn GetDataHere(
            &self,
            _asked: *const FORMATETC,
            _medium: *mut STGMEDIUM,
        ) -> windows::core::Result<()> {
            Err(E_NOTIMPL.into())
        }

        fn QueryGetData(&self, asked: *const FORMATETC) -> HRESULT {
            // SAFETY: OLE passes a valid FORMATETC for the call.
            match unsafe { asked.as_ref() }.and_then(|asked| self.offered(asked)) {
                Some(_) => S_OK,
                None => DV_E_FORMATETC,
            }
        }

        fn GetCanonicalFormatEtc(&self, _asked: *const FORMATETC, _out: *mut FORMATETC) -> HRESULT {
            E_NOTIMPL
        }

        fn SetData(
            &self,
            _format: *const FORMATETC,
            _medium: *const STGMEDIUM,
            _release: BOOL,
        ) -> windows::core::Result<()> {
            Err(E_NOTIMPL.into())
        }

        fn EnumFormatEtc(&self, direction: u32) -> windows::core::Result<IEnumFORMATETC> {
            if direction != u32::try_from(DATADIR_GET.0).expect("positive") {
                return Err(E_NOTIMPL.into());
            }
            let formats = self.formats.map(|(_, format)| format);
            // SAFETY: the enumerator copies the formats it is given.
            unsafe { SHCreateStdEnumFmtEtc(&formats) }
        }

        fn DAdvise(
            &self,
            _format: *const FORMATETC,
            _flags: u32,
            _sink: Ref<IAdviseSink>,
        ) -> windows::core::Result<u32> {
            Err(OLE_E_ADVISENOTSUPPORTED.into())
        }

        fn DUnadvise(&self, _connection: u32) -> windows::core::Result<()> {
            Err(OLE_E_ADVISENOTSUPPORTED.into())
        }

        fn EnumDAdvise(&self) -> windows::core::Result<IEnumSTATDATA> {
            Err(OLE_E_ADVISENOTSUPPORTED.into())
        }
    }

    /// The source side of the drag loop: Esc cancels it and releasing the left button drops.
    #[implement(IDropSource)]
    pub(super) struct Source;

    #[allow(non_snake_case)]
    impl IDropSource_Impl for Source_Impl {
        fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
            if escape.as_bool() {
                return DRAGDROP_S_CANCEL;
            }
            if keys & MK_LBUTTON == MODIFIERKEYS_FLAGS(0) {
                return DRAGDROP_S_DROP;
            }
            S_OK
        }

        fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
            DRAGDROP_S_USEDEFAULTCURSORS
        }
    }
}

use objects::{Offer, Source};

#[cfg(test)]
mod tests {
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    use windows::Win32::System::Ole::{CF_HDROP, ReleaseStgMedium};

    use super::*;

    fn dragged(name: &str) -> Dragged {
        Dragged {
            name: name.to_owned(),
            png: b"\x89PNG\r\n\x1a\n and the rest".to_vec(),
            bitmap: vec![40, 0, 0, 0, 7, 7],
        }
    }

    /// The bytes of `medium`'s global block, then releases it. Windows may round a block up,
    /// so only the first `length` bytes are kept.
    fn global_bytes(mut medium: STGMEDIUM, length: usize) -> Vec<u8> {
        // SAFETY: GetData returned an HGLOBAL medium, which is read under its lock and then
        // released once.
        unsafe {
            let block = medium.u.hGlobal;
            assert!(GlobalSize(block) >= length);
            let bytes = std::slice::from_raw_parts(GlobalLock(block).cast::<u8>(), length).to_vec();
            // GlobalUnlock reports an error once the lock count reaches zero, which is
            // this call's purpose.
            let _ = GlobalUnlock(block);
            ReleaseStgMedium(&raw mut medium);
            bytes
        }
    }

    #[test]
    fn a_drag_offers_a_named_png_file_from_memory_the_png_and_the_bitmap_and_no_file_path() {
        let name = "Screenshot 2026-11-02 at 10.00.00.png";
        let wanted = dragged(name);
        let data: IDataObject = Offer::new(dragged(name)).expect("the offer is made").into();
        for offered in OFFERED {
            let asked = format(offered).expect("the format registers");
            // SAFETY: `asked` lives for the call.
            let answer = unsafe { data.QueryGetData(&raw const asked) };
            assert_eq!(answer, S_OK, "{offered:?}");
        }
        let hdrop = FORMATETC {
            cfFormat: CF_HDROP.0,
            ..format(Offered::Png).expect("PNG registers")
        };
        // SAFETY: `hdrop` lives for the call.
        let answer = unsafe { data.QueryGetData(&raw const hdrop) };
        assert_eq!(answer, DV_E_FORMATETC);
        let get = |offered| {
            let asked = format(offered).expect("the format registers");
            // SAFETY: `asked` lives for the call.
            unsafe { data.GetData(&raw const asked) }.expect("the data is offered")
        };
        assert_eq!(
            global_bytes(get(Offered::Png), wanted.png.len()),
            wanted.png
        );
        assert_eq!(
            global_bytes(get(Offered::Bitmap), wanted.bitmap.len()),
            wanted.bitmap
        );
        let descriptor = global_bytes(
            get(Offered::FileDescriptor),
            size_of::<FILEGROUPDESCRIPTORW>(),
        );
        // SAFETY: the bytes are one FILEGROUPDESCRIPTORW, read unaligned as it is packed.
        let group = unsafe {
            descriptor
                .as_ptr()
                .cast::<FILEGROUPDESCRIPTORW>()
                .read_unaligned()
        };
        let [file] = { group.fgd };
        let (items, size, units) = (group.cItems, file.nFileSizeLow, { file.cFileName });
        assert_eq!(
            (items, crate::text_before_nul(&units), size),
            (
                1,
                name.to_owned(),
                u32::try_from(wanted.png.len()).expect("small")
            )
        );
        let mut contents = get(Offered::FileContents);
        let mut read = vec![0_u8; wanted.png.len() + 8];
        let mut count = 0;
        // SAFETY: the medium is an IStream; `read` is writable for its length, and the medium
        // is released once.
        unsafe {
            let stream = contents.u.pstm.as_ref().expect("a stream");
            // The bytes read, counted in `count`, are what the test checks.
            let _ = stream.Read(
                read.as_mut_ptr().cast(),
                u32::try_from(read.len()).expect("small"),
                Some(&raw mut count),
            );
            ReleaseStgMedium(&raw mut contents);
        }
        read.truncate(usize::try_from(count).expect("small"));
        assert_eq!(read, wanted.png);
    }

    #[test]
    fn a_target_that_keeps_the_drag_object_cannot_read_the_image_after_the_drag() {
        let asked = format(Offered::Png).expect("PNG registers");
        let wanted = dragged("a.png");
        let kept = offered_during(dragged("a.png"), |data| {
            // SAFETY: `asked` lives for the call.
            let medium = unsafe { data.GetData(&raw const asked) }.expect("read during the drag");
            assert_eq!(global_bytes(medium, wanted.png.len()), wanted.png);
            data.clone()
        })
        .expect("the offer is made");
        // SAFETY: `asked` lives for the call.
        let after = unsafe { kept.GetData(&raw const asked) };
        assert_eq!(
            after.err().map(|error| error.code()),
            Some(OLE_E_NOTRUNNING)
        );
    }

    #[test]
    fn a_name_too_long_for_a_file_descriptor_is_refused() {
        assert!(Offer::new(dragged(&"a".repeat(259))).is_ok());
        assert!(Offer::new(dragged(&"a".repeat(260))).is_err());
    }

    #[test]
    fn esc_cancels_the_drag_and_releasing_the_left_button_drops_it() {
        let source: IDropSource = Source.into();
        // SAFETY: each call takes plain values.
        let asked = |escape: bool, keys| unsafe { source.QueryContinueDrag(escape, keys) };
        assert_eq!(asked(true, MK_LBUTTON), DRAGDROP_S_CANCEL);
        assert_eq!(asked(false, MODIFIERKEYS_FLAGS(0)), DRAGDROP_S_DROP);
        assert_eq!(asked(false, MK_LBUTTON), S_OK);
    }
}
