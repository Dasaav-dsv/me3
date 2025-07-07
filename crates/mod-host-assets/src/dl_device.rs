use std::{
    borrow::Cow,
    collections::VecDeque,
    fmt, mem,
    ops::Range,
    ptr::{self, NonNull},
};

use cxx_stl::{
    alloc::WithCxxProxy,
    string::utf16::RawUtf16String,
    vec::{CxxVecLayout, RawVec},
};
use thiserror::Error;
use windows::Win32::System::Threading::{
    EnterCriticalSection, LeaveCriticalSection, CRITICAL_SECTION,
};

use crate::{
    alloc::DlStdAllocator,
    pe,
    string::{DlUtf16StringLayout, EncodingError},
};

pub type DlDeviceManager = DlDeviceManagerLayout<
    cxx_stl::vec::Layout<DlStdAllocator>,
    cxx_stl::string::utf16::Layout<DlStdAllocator>,
>;

pub type DlDeviceManagerMsvc2012 = DlDeviceManagerLayout<
    cxx_stl::vec::msvc2012::Layout<DlStdAllocator>,
    cxx_stl::string::utf16::msvc2012::Layout<DlStdAllocator>,
>;

#[repr(C)]
pub struct DlDeviceManagerLayout<L1, L2>
where
    L1: WithCxxProxy<Value = RawVec, Alloc = DlStdAllocator>,
    L2: WithCxxProxy<Value = RawUtf16String, Alloc = DlStdAllocator>,
{
    devices: CxxVecLayout<NonNull<DlDevice<L2>>, DlStdAllocator, L1>,
    spis: CxxVecLayout<NonNull<u8>, DlStdAllocator, L1>,
    disk_device: NonNull<DlDevice<L2>>,
    virtual_roots: CxxVecLayout<DlVirtualRoot<L2>, DlStdAllocator, L1>,
    bnd3_mounts: CxxVecLayout<DlVirtualMount<L2>, DlStdAllocator, L1>,
    bnd4_mounts: CxxVecLayout<DlVirtualMount<L2>, DlStdAllocator, L1>,
    bnd3_spi: NonNull<u8>,
    bnd4_spi: NonNull<u8>,
    mutex_vtable: usize,
    critical_section: CRITICAL_SECTION,
}

pub type DlDevice<L> = NonNull<DlDeviceVtable<L>>;

pub type DlFileOperator = DlFileOperatorLayout<cxx_stl::string::utf16::Layout<DlStdAllocator>>;
pub type DlFileOperatorMsvc2012 =
    DlFileOperatorLayout<cxx_stl::string::utf16::msvc2012::Layout<DlStdAllocator>>;

pub type DlFileOperatorLayout<L> = NonNull<DlFileOperatorVtable<L>>;

#[repr(C)]
pub struct DlVirtualRoot<L>
where
    L: WithCxxProxy<Value = RawUtf16String, Alloc = DlStdAllocator>,
{
    root: DlUtf16StringLayout<DlStdAllocator, L>,
    expanded: DlUtf16StringLayout<DlStdAllocator, L>,
}

#[repr(C)]
pub struct DlVirtualMount<L>
where
    L: WithCxxProxy<Value = RawUtf16String, Alloc = DlStdAllocator>,
{
    root: DlUtf16StringLayout<DlStdAllocator, L>,
    device: NonNull<DlDevice<L>>,
    size: usize,
}

pub struct DlDeviceManagerGuard<L1, L2>
where
    L1: WithCxxProxy<Value = RawVec, Alloc = DlStdAllocator>,
    L2: WithCxxProxy<Value = RawUtf16String, Alloc = DlStdAllocator>,
{
    inner: NonNull<DlDeviceManagerLayout<L1, L2>>,
}

type DlDeviceOpen<L> = unsafe extern "C" fn(
    NonNull<DlDevice<L>>,
    path: NonNull<DlUtf16StringLayout<DlStdAllocator, L>>,
    path_cstr: *const u16,
    NonNull<u8>,
    DlStdAllocator,
    bool,
) -> Option<NonNull<DlFileOperatorLayout<L>>>;

#[repr(C)]
pub struct DlDeviceVtable<L>
where
    L: WithCxxProxy<Value = RawUtf16String, Alloc = DlStdAllocator>,
{
    _dtor: usize,
    open_file: DlDeviceOpen<L>,
}

type DlFileOperatorSetPath<L> = unsafe extern "C" fn(
    NonNull<DlFileOperatorLayout<L>>,
    path: NonNull<DlUtf16StringLayout<DlStdAllocator, L>>,
    bool,
    bool,
) -> bool;

#[repr(C)]
pub struct DlFileOperatorVtable<L>
where
    L: WithCxxProxy<Value = RawUtf16String, Alloc = DlStdAllocator>,
{
    _dtor: usize,
    _copy: usize,
    pub set_path: DlFileOperatorSetPath<L>,
    pub set_path2: DlFileOperatorSetPath<L>,
    pub set_path3: DlFileOperatorSetPath<L>,
}

pub struct BndSnapshot {
    inner: Vec<Vec<u16>>,
}

pub type VfsMounts = VfsMountsLayout<cxx_stl::string::utf16::Layout<DlStdAllocator>>;

pub type VfsMountsMsvc2012 =
    VfsMountsLayout<cxx_stl::string::utf16::msvc2012::Layout<DlStdAllocator>>;

pub struct VfsMountsLayout<L>
where
    L: WithCxxProxy<Value = RawUtf16String, Alloc = DlStdAllocator>,
{
    inner: Vec<DlVirtualMount<L>>,
}

pub struct VfsPushGuard<'a, L1, L2>
where
    L1: WithCxxProxy<Value = RawVec, Alloc = DlStdAllocator>,
    L2: WithCxxProxy<Value = RawUtf16String, Alloc = DlStdAllocator>,
{
    owner: &'a mut DlDeviceManagerGuard<L1, L2>,
    old_len: usize,
}

impl<L1, L2> DlDeviceManagerLayout<L1, L2>
where
    L1: WithCxxProxy<Value = RawVec, Alloc = DlStdAllocator>,
    L2: WithCxxProxy<Value = RawUtf16String, Alloc = DlStdAllocator>,
{
    pub fn lock(ptr: NonNull<Self>) -> DlDeviceManagerGuard<L1, L2> {
        unsafe {
            EnterCriticalSection(&raw mut (*ptr.as_ptr()).critical_section);
        }

        DlDeviceManagerGuard { inner: ptr }
    }
}

impl DlDeviceManager {
    /// # Safety
    /// [`pelite::pe64::PeView::module`] must be safe to call on `image_base`
    pub unsafe fn find_device_manager(image_base: *const u8) -> Result<NonNull<Self>, FindError> {
        // SAFETY: must be upheld by caller.
        let [data, rdata] = unsafe { pe::sections(image_base, [".data", ".rdata"])? };

        const SIZE: usize = mem::size_of::<*const u8>();
        const ALIGNMENT: usize = mem::align_of::<*const u8>();

        let data_range = data.as_ptr_range();

        let Range {
            start,
            end: data_end,
        } = data_range;

        let mut data_ptr =
            start.wrapping_byte_offset(start.align_offset(ALIGNMENT) as isize - SIZE as isize);

        while data_ptr < data_end {
            data_ptr = data_ptr.wrapping_byte_add(SIZE);

            // SAFETY: pointer is aligned and non-null.
            let manager_ptr = unsafe { data_ptr.cast::<*const Self>().read() };

            if !data_range.contains(&unsafe { manager_ptr.add(1).cast::<u8>().sub(1) })
                || !data_range.contains(&manager_ptr.cast())
            {
                continue;
            }

            // SAFETY: pointer is in bounds of ".data".
            if Self::verify_dl_device_manager_layout(
                manager_ptr,
                data_range.clone(),
                rdata.as_ptr_range(),
            ) {
                return Ok(NonNull::new(manager_ptr as _).unwrap());
            }
        }

        Err(FindError::Instance)
    }

    /// # Safety
    /// `ptr` must be in bounds for all reads.
    fn verify_dl_device_manager_layout(
        device_manager: *const Self,
        data_range: Range<*const u8>,
        rdata_range: Range<*const u8>,
    ) -> bool {
        if !device_manager.is_aligned() {
            return false;
        }

        let ptr = device_manager.cast::<*const usize>();

        macro_rules! verify_vec {
            ($v:expr, $alloc:expr) => {
                #[allow(unused_unsafe)]
                unsafe {
                    if $alloc != $v.read() {
                        return false;
                    }

                    let first = $v.add(1).read();
                    let last = $v.add(2).read();
                    let end = $v.add(3).read();

                    if !first.is_aligned() || !last.is_aligned() || !end.is_aligned() {
                        return false;
                    }

                    if first > last || last > end {
                        return false;
                    }
                }
            };
        }

        // SAFETY: pointer is aligned for all reads, in bounds by precondition.
        unsafe {
            let alloc = ptr.read();

            if !alloc.is_aligned() || !data_range.contains(&alloc.cast()) {
                return false;
            }

            verify_vec!(ptr, alloc);

            verify_vec!(
                &raw const (*device_manager).spis as *const *const usize,
                alloc
            );

            verify_vec!(
                &raw const (*device_manager).virtual_roots as *const *const usize,
                alloc
            );

            verify_vec!(
                &raw const (*device_manager).bnd3_mounts as *const *const usize,
                alloc
            );

            verify_vec!(
                &raw const (*device_manager).bnd4_mounts as *const *const usize,
                alloc
            );

            let disk_device =
                ptr::read(&raw const (*device_manager).disk_device as *const *const usize);

            if disk_device.is_null() || !disk_device.is_aligned() {
                return false;
            }

            let bnd3_spi = ptr::read(&raw const (*device_manager).bnd3_spi as *const *const usize);

            if bnd3_spi.is_null() || !bnd3_spi.is_aligned() {
                return false;
            }

            let bnd4_spi = ptr::read(&raw const (*device_manager).bnd4_spi as *const *const usize);

            if bnd4_spi.is_null() || !bnd4_spi.is_aligned() {
                return false;
            }

            let mutex_vtable =
                ptr::read(&raw const (*device_manager).mutex_vtable as *const *const usize);

            if !mutex_vtable.is_aligned() || !rdata_range.contains(&mutex_vtable.cast()) {
                return false;
            }
        }

        true
    }
}

impl DlDeviceManagerMsvc2012 {
    /// # Safety
    /// [`pelite::pe64::PeView::module`] must be safe to call on `image_base`
    pub unsafe fn find_device_manager(image_base: *const u8) -> Result<NonNull<Self>, FindError> {
        // SAFETY: must be upheld by caller.
        let [data, rdata] = unsafe { pe::sections(image_base, [".data", ".rdata"])? };

        const SIZE: usize = mem::size_of::<*const u8>();
        const ALIGNMENT: usize = mem::align_of::<*const u8>();

        let data_range = data.as_ptr_range();

        let Range {
            start,
            end: data_end,
        } = data_range;

        let mut data_ptr =
            start.wrapping_byte_offset(start.align_offset(ALIGNMENT) as isize - SIZE as isize);

        while data_ptr < data_end {
            data_ptr = data_ptr.wrapping_byte_add(SIZE);

            // SAFETY: pointer is aligned and non-null.
            let manager_ptr = unsafe { data_ptr.cast::<*const Self>().read() };

            if !data_range.contains(&unsafe { manager_ptr.add(1).cast::<u8>().sub(1) })
                || !data_range.contains(&manager_ptr.cast())
            {
                continue;
            }

            // SAFETY: pointer is in bounds of ".data".
            if Self::verify_dl_device_manager_layout(
                manager_ptr,
                data_range.clone(),
                rdata.as_ptr_range(),
            ) {
                return Ok(NonNull::new(manager_ptr as _).unwrap());
            }
        }

        Err(FindError::Instance)
    }

    /// # Safety
    /// `ptr` must be in bounds for all reads.
    fn verify_dl_device_manager_layout(
        device_manager: *const Self,
        data_range: Range<*const u8>,
        rdata_range: Range<*const u8>,
    ) -> bool {
        if !device_manager.is_aligned() {
            return false;
        }

        let ptr = device_manager.cast::<*const usize>();

        macro_rules! verify_vec {
            ($v:expr, $alloc:expr) => {
                #[allow(unused_unsafe)]
                unsafe {
                    if $alloc != $v.add(3).read() {
                        return false;
                    }

                    let first = $v.read();
                    let last = $v.add(1).read();
                    let end = $v.add(2).read();

                    if !first.is_aligned() || !last.is_aligned() || !end.is_aligned() {
                        return false;
                    }

                    if first > last || last > end {
                        return false;
                    }
                }
            };
        }

        // SAFETY: pointer is aligned for all reads, in bounds by precondition.
        unsafe {
            let alloc = ptr.add(3).read();

            if !alloc.is_aligned() || !data_range.contains(&alloc.cast()) {
                return false;
            }

            verify_vec!(ptr, alloc);

            verify_vec!(
                &raw const (*device_manager).spis as *const *const usize,
                alloc
            );

            verify_vec!(
                &raw const (*device_manager).virtual_roots as *const *const usize,
                alloc
            );

            verify_vec!(
                &raw const (*device_manager).bnd3_mounts as *const *const usize,
                alloc
            );

            verify_vec!(
                &raw const (*device_manager).bnd4_mounts as *const *const usize,
                alloc
            );

            let disk_device =
                ptr::read(&raw const (*device_manager).disk_device as *const *const usize);

            if disk_device.is_null() || !disk_device.is_aligned() {
                return false;
            }

            let bnd3_spi = ptr::read(&raw const (*device_manager).bnd3_spi as *const *const usize);

            if bnd3_spi.is_null() || !bnd3_spi.is_aligned() {
                return false;
            }

            let bnd4_spi = ptr::read(&raw const (*device_manager).bnd4_spi as *const *const usize);

            if bnd4_spi.is_null() || !bnd4_spi.is_aligned() {
                return false;
            }

            let mutex_vtable =
                ptr::read(&raw const (*device_manager).mutex_vtable as *const *const usize);

            if !mutex_vtable.is_aligned() || !rdata_range.contains(&mutex_vtable.cast()) {
                return false;
            }
        }

        true
    }
}

impl<L1, L2> DlDeviceManagerGuard<L1, L2>
where
    L1: WithCxxProxy<Value = RawVec, Alloc = DlStdAllocator>,
    L2: WithCxxProxy<Value = RawUtf16String, Alloc = DlStdAllocator>,
{
    pub fn snapshot(&self) -> Result<BndSnapshot, EncodingError> {
        let device_manager = unsafe { self.inner.as_ref() };

        let snapshot = device_manager
            .bnd4_mounts
            .iter()
            .map(|m| m.root.get().map(|s| s.as_bytes().to_owned()))
            .collect::<Result<Vec<Vec<u16>>, EncodingError>>()?;

        Ok(BndSnapshot::new(snapshot))
    }

    pub fn extract_new(&mut self, snapshot: BndSnapshot) -> VfsMountsLayout<L2> {
        let device_manager = unsafe { self.inner.as_mut() };

        let mut removed_mounts = VecDeque::new();

        for i in (0..device_manager.bnd4_mounts.len()).rev() {
            if !snapshot.has_mount(&device_manager.bnd4_mounts[i]) {
                removed_mounts.push_front(device_manager.bnd4_mounts.remove(i));
            }
        }

        VfsMountsLayout {
            inner: removed_mounts.into(),
        }
    }

    pub fn push_vfs(&mut self, vfs: &VfsMountsLayout<L2>) -> VfsPushGuard<'_, L1, L2> {
        let device_manager = unsafe { self.inner.as_mut() };

        let old_len = device_manager.bnd4_mounts.len();

        device_manager.bnd4_mounts.extend(vfs.inner.clone());

        VfsPushGuard {
            owner: self,
            old_len,
        }
    }

    pub fn expand_path<'a>(&self, path: &'a [u16]) -> Cow<'a, [u16]> {
        let device_manager = unsafe { self.inner.as_ref() };

        let mut expanded = Cow::Borrowed(path);

        loop {
            let Some(root_end) = expanded.windows(2).position(is_root_separator) else {
                break;
            };

            let root = &expanded[..root_end];

            let virtual_root = device_manager
                .virtual_roots
                .iter()
                .find(|v| v.root.get().is_ok_and(|r| root == r.as_bytes()));

            if let Some(replace_with) = virtual_root.and_then(|v| v.expanded.get().ok()) {
                let mut new = replace_with.as_bytes().to_owned();
                new.extend_from_slice(&expanded[root_end + 2..]);
                expanded = Cow::Owned(new);
            } else {
                break;
            }
        }

        expanded
    }

    pub fn open_disk_file(&self) -> DlDeviceOpen<L2> {
        unsafe {
            let device_manager = self.inner.as_ref();
            device_manager.disk_device.as_ref().as_ref().open_file
        }
    }
}

impl BndSnapshot {
    fn new(vec: Vec<Vec<u16>>) -> Self {
        let mut sorted = vec;
        sorted.sort_unstable();
        Self { inner: sorted }
    }

    fn has_mount<L>(&self, mount: &DlVirtualMount<L>) -> bool
    where
        L: WithCxxProxy<Value = RawUtf16String, Alloc = DlStdAllocator>,
    {
        mount.root.get().is_ok_and(|r| {
            self.inner
                .binary_search_by(|v| Ord::cmp(&**v, r.as_bytes()))
                .is_ok()
        })
    }
}

impl<L> VfsMountsLayout<L>
where
    L: WithCxxProxy<Value = RawUtf16String, Alloc = DlStdAllocator>,
{
    pub const fn new() -> Self {
        Self { inner: Vec::new() }
    }

    pub fn open_disk_file_fn(&self) -> Option<DlDeviceOpen<L>> {
        unsafe {
            let ptr = self.inner.first()?.device;
            Some(ptr::read(&raw const ptr.read().as_ref().open_file))
        }
    }

    pub fn append(&mut self, new: Self) {
        let mut inner = new.inner;
        self.inner.append(&mut inner);
    }

    /// # Safety
    /// only if passed arguments from `DlDeviceOpen`.
    pub unsafe fn try_open_file(
        &self,
        path: NonNull<DlUtf16StringLayout<DlStdAllocator, L>>,
        path_cstr: *const u16,
        container: NonNull<u8>,
        allocator: DlStdAllocator,
        is_temp_file: bool,
    ) -> Option<NonNull<DlFileOperatorLayout<L>>> {
        let path_bytes = unsafe { path.as_ref().get().ok()?.as_bytes() };

        let root_end = path_bytes.windows(2).position(is_root_separator)?;
        let root = &path_bytes[..root_end];

        self.inner
            .iter()
            .find(|m| m.root.get().is_ok_and(|r| root == r.as_bytes()))
            .and_then(|m| {
                let f = unsafe { ptr::read(&raw const m.device.read().as_ref().open_file) };
                unsafe {
                    f(
                        m.device,
                        path,
                        path_cstr,
                        container,
                        allocator,
                        is_temp_file,
                    )
                }
            })
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

fn is_root_separator(w: &[u16]) -> bool {
    w[0] == ':' as u16 && w[1] == '/' as u16
}

#[derive(Clone, Debug, Error)]
pub enum FindError {
    #[error("{0}")]
    PeSection(pe::SectionError),
    #[error("DlDeviceManager instance not found")]
    Instance,
}

impl From<pe::SectionError> for FindError {
    fn from(value: pe::SectionError) -> Self {
        FindError::PeSection(value)
    }
}

impl<L1, L2> Drop for DlDeviceManagerGuard<L1, L2>
where
    L1: WithCxxProxy<Value = RawVec, Alloc = DlStdAllocator>,
    L2: WithCxxProxy<Value = RawUtf16String, Alloc = DlStdAllocator>,
{
    fn drop(&mut self) {
        unsafe {
            LeaveCriticalSection(&mut self.inner.as_mut().critical_section);
        }
    }
}

impl<L1, L2> Drop for VfsPushGuard<'_, L1, L2>
where
    L1: WithCxxProxy<Value = RawVec, Alloc = DlStdAllocator>,
    L2: WithCxxProxy<Value = RawUtf16String, Alloc = DlStdAllocator>,
{
    fn drop(&mut self) {
        unsafe {
            self.owner.inner.as_mut().bnd4_mounts.truncate(self.old_len);
        }
    }
}

impl<L> Clone for DlVirtualMount<L>
where
    L: WithCxxProxy<Value = RawUtf16String, Alloc = DlStdAllocator>,
{
    fn clone(&self) -> Self {
        Self {
            root: self.root.clone(),
            device: self.device,
            size: self.size,
        }
    }
}

impl<L> fmt::Debug for DlVirtualMount<L>
where
    L: WithCxxProxy<Value = RawUtf16String, Alloc = DlStdAllocator>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DlVirtualMount")
            .field("root", &self.root.get().map(|r| r.to_string()))
            .field("device", &self.device)
            .finish()
    }
}

impl<L> fmt::Debug for VfsMountsLayout<L>
where
    L: WithCxxProxy<Value = RawUtf16String, Alloc = DlStdAllocator>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VfsMountsLayout")
            .field("inner", &self.inner)
            .finish()
    }
}

impl<L> Default for VfsMountsLayout<L>
where
    L: WithCxxProxy<Value = RawUtf16String, Alloc = DlStdAllocator>,
{
    fn default() -> Self {
        Self { inner: vec![] }
    }
}

unsafe impl<L> Send for VfsMountsLayout<L> where
    L: WithCxxProxy<Value = RawUtf16String, Alloc = DlStdAllocator>
{
}

unsafe impl<L> Sync for VfsMountsLayout<L> where
    L: WithCxxProxy<Value = RawUtf16String, Alloc = DlStdAllocator>
{
}
