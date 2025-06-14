use std::ptr::{self, NonNull};

use crate::string::DlUtf16HashString;

#[repr(C)]
pub struct CsFile {
    vtable: NonNull<CsFileVtable>,
}

#[repr(C)]
pub struct CsFileVtable {
    _rtclass: usize,
    _dtor: usize,
    _get_rescap: CsFileGetRescap,
    process_filecap: CsFileProcessFilecap,
}

pub type CsFileGetRescap = extern "C" fn(NonNull<CsFile>, name: *mut DlUtf16HashString);

pub type CsFileProcessFilecap =
    extern "C" fn(NonNull<CsFile>, path: NonNull<DlUtf16HashString>, usize, u32) -> usize;

impl CsFile {
    pub fn process_filecap_fn() -> Option<CsFileProcessFilecap> {
        let map = from_singleton::map();

        // SAFETY: pointers are aligned and initialized in the game's context,
        // no references are created, see `std::ptr::addr_of`.
        unsafe {
            let instance = map
                .get("CSFile")
                .or_else(|| map.get("SprjFile"))
                .and_then(|p| NonNull::new(p.as_ref().cast::<Self>()))?;

            let vtable = ptr::read(&raw const instance.as_ref().vtable);

            Some(ptr::read(&raw const vtable.as_ref().process_filecap))
        }
    }
}
