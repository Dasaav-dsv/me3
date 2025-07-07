use std::{
    ffi::OsString,
    mem,
    os::windows::ffi::OsStringExt,
    ptr::NonNull,
    sync::{Arc, Mutex, OnceLock},
};

use either::Either;
use eyre::eyre;
use me3_mod_host_assets::{
    dl_device::{
        self, DlDeviceManager, DlDeviceManagerLayout, DlDeviceManagerMsvc2012, VfsMountsLayout,
    },
    dlc::mount_dlc_ebl,
    ebl::EblFileManager,
    file_step,
    mapping::ArchiveOverrideMapping,
    string::DlUtf16StringLayout,
    wwise::{self, find_wwise_open_file, AkOpenMode},
};
use me3_mod_protocol::Game;
use tracing::{debug, error, info, info_span, instrument, warn};
use windows::core::PCWSTR;

use crate::host::ModHost;

#[instrument(name = "assets", skip_all)]
pub fn attach_override(mapping: Arc<ArchiveOverrideMapping>) -> Result<(), eyre::Error> {
    let attach_context = AttachContext::new(mapping);
    Arc::new(attach_context).attach()
}

impl AttachContext {
    fn new(mapping: Arc<ArchiveOverrideMapping>) -> Self {
        let host = ModHost::get_attached();

        Self {
            game: host.game(),
            image_base: host.image_base(),
            mapping,
            device_manager: OnceLock::new(),
        }
    }

    fn attach(self: Arc<Self>) -> Result<(), eyre::Error> {
        self.enable_loose_params();

        self.clone().hook_file_device()?;

        if let Err(e) = self.clone().try_hook_wwise() {
            warn!("error" = &*e, "skipping Wwise hook");
        }

        Ok(())
    }

    // Hook lifecycle for asset overrides
    //
    // 1. `FileStep::STEP_Init` This is where the game's file loading pipeline is set up, with
    //    prerequisites already initialized (like DlDeviceManager, which we need). The first hook is
    //    placed here, which allows for calling the trampoline in the middle of the initialization
    //    to capture and extract the DVDBNDs after they are mounted.
    //
    // 2.1. `DlMicrosoftDiskFileDevice::Open`
    //     Its address is acquired from DlDeviceManager (see above for initialization guarantees).
    //     This function handles opening on-disk files, but will also be called with any
    //     file not present in DVDBND roots. By removing all DVDBNDs (see 1.) all files fall
    //     through to this point and can be dispatched to actual on-disk file overrides
    //     or the original files in the archives via `VfsMountsLayout::try_open_file`.
    //     Returns a `DLFileOperator` for the opened file.
    //
    // 2.2. `DLFileOperator::SetPath`
    //     Acquired from a `DLFileOperator` vtable returned from 2.1. and hooked alongside it,
    //     otherwise the returned overriden paths will be themselves overriden by the game.
    //
    // 3. Encrypted Binder Light Without hooking `make_ebl_object` overriden files will be attempted
    //    to be decrypted by the game as if loaded from the DVDBNDs. This hook prevents DLEBL
    //    objects from being created for overriden files.
    //
    // 4. `CSDlcPlatformImp_forSteam::Mount` DLC archives like DLC1, DLC2, ... BDTs/BHDs are loaded
    //    at a later point after a DLC license check. They will not be captured in 1., so this hook
    //    is needed to manipulate them for overriding files from the DLC DVDBNDs.
    #[instrument(name = "file_device", skip_all)]
    fn hook_file_device(self: Arc<Self>) -> Result<(), eyre::Error> {
        let init_fn = unsafe { file_step::find_init_fn(self.image_base)? };

        debug!("FileStep::STEP_Init" = ?init_fn);
        debug!("FileStep::STEP_Init" = ?init_fn);

        ModHost::get_attached()
            .hook(init_fn)
            .with_span(info_span!("hook"))
            .with_closure(move |p1, trampoline| {
                let device_manager = match self.locate_device_manager() {
                    Ok(device_manager) => device_manager,
                    Err(e) => {
                        error!("error" = &*eyre!(e), "failed to locate device manager");

                        unsafe {
                            trampoline(p1);
                        }

                        return;
                    }
                };

                // Dispatch to one of two container allocator layouts,
                // which correspond to MSVC2012 (for Dark Souls 3) or later (for other games).
                either::for_both!(device_manager, device_manager_ptr => {
                    let mut device_manager = DlDeviceManagerLayout::lock(device_manager_ptr);

                    let open_disk_file = device_manager.open_disk_file();

                    // Closure that fetches file overrides from the `ArchiveOverrideMapping`
                    // for virtual paths expanded via `DlDeviceManagerGuard::expand_path`.
                    let override_path = {
                        let mapping = self.mapping.clone();

                        move |path, enable_logging| {
                            let path = DlUtf16StringLayout::get(path).ok()?;
                            let expanded =
                                DlDeviceManagerLayout::lock(device_manager_ptr).expand_path(path.as_bytes());

                            let expanded = OsString::from_wide(&expanded);

                            if enable_logging {
                                debug!("asset" = ?expanded);
                            }

                            let (mapped_path, mapped_override) =
                                mapping.vfs_override(expanded)?;

                            if enable_logging {
                                info!("override" = mapped_path);
                            }

                            let mut path = path.clone();
                            path.replace(mapped_override);

                            Some(path)
                        }
                    };

                    let vfs_mounts = Arc::new(Mutex::new(Default::default()));

                    // 2.1.
                    let result = ModHost::get_attached()
                        .hook(open_disk_file)
                        .with_closure({
                            let vfs_mounts = vfs_mounts.clone();

                            move |p1, path, p3, p4, p5, p6, trampoline| {
                                let file_operator = if let Some(path) = override_path(unsafe { path.as_ref() }, true)
                                {
                                    unsafe {
                                        trampoline(
                                            p1,
                                            NonNull::from(&path).cast(),
                                            path.as_ptr(),
                                            p4,
                                            p5.clone(),
                                            p6,
                                        )
                                    }
                                } else {
                                    unsafe { trampoline(p1, path, p3, p4, p5.clone(), p6) }
                                };

                                // 2.2.
                                if let Some(file_operator) = file_operator {
                                    static HOOK_RESULT: OnceLock<bool> = OnceLock::new();

                                    if *HOOK_RESULT.get_or_init(|| {
                                        let vtable = unsafe { file_operator.as_ref().as_ref() };

                                        // There are three similar set path function overloads,
                                        // hooking just the first one is insufficient for some games (e.g. Nightreign).
                                        for set_path in [vtable.set_path, vtable.set_path2, vtable.set_path3] {
                                            let override_path = override_path.clone();

                                            let result = ModHost::get_attached()
                                                .hook(set_path)
                                                .with_closure(move |p1, path, p3, p4, trampoline| {
                                                    if let Some(path) = override_path(unsafe { path.as_ref() }, false) {
                                                        unsafe { trampoline(p1, path.as_ref().into(), p3, p4) }
                                                    } else {
                                                        unsafe { trampoline(p1, path, p3, p4) }
                                                    }
                                                })
                                                .install();

                                            if let Err(e) = result {
                                                error!("error" = %e, "failed to hook DLFileOperator::SetPath: {e}");
                                                return false;
                                            }
                                        }

                                        true
                                    })
                                    {
                                        return Some(file_operator);
                                    }
                                }

                                // Open non-overriden file from the game archives.
                                unsafe { VfsMountsLayout::try_open_file(&*vfs_mounts.lock().unwrap(), path, p3, p4, p5, p6) }
                            }
                        })
                        .install();

                    if let Err(e) = result {
                        error!("error" = %e, "failed to hook device manager");

                        unsafe {
                            trampoline(p1);
                        }

                        return;
                    } else {
                        info!("kind" = "device_manager", "applied asset override hook");
                    }

                    // 1.
                    let snap = device_manager.snapshot();

                    unsafe {
                        trampoline(p1);
                    }

                    match snap {
                        Ok(snap) => {
                            let new = device_manager.extract_new(snap);

                            debug!("extracted_mounts" = ?new);

                            *vfs_mounts.lock().unwrap() = new;

                            // 3.
                            let make_ebl_object = unsafe { EblFileManager::make_ebl_object(self.image_base).map_err(|e| eyre!(e)) };

                            let result = make_ebl_object.and_then(|make_ebl_object| {
                                debug!(?make_ebl_object);

                                ModHost::get_attached()
                                    .hook(make_ebl_object)
                                    .with_closure({
                                        let mapping = self.mapping.clone();
                                        let vfs_mounts = vfs_mounts.clone();

                                        move |p1, path, p3, trampoline| {
                                            let mut device_manager = DlDeviceManagerLayout::lock(device_manager_ptr);

                                            let path_cstr = PCWSTR::from_raw(path);
                                            let expanded = unsafe { device_manager.expand_path(path_cstr.as_wide()) };

                                            if mapping
                                                .vfs_override(OsString::from_wide(&expanded))
                                                .is_some()
                                            {
                                                return None;
                                            }

                                            let _guard = device_manager.push_vfs(&*vfs_mounts.lock().unwrap());

                                            unsafe { (trampoline)(p1, path, p3) }
                                        }
                                    })
                                    .install().map_err(|e| eyre!(e))
                            });

                            if let Err(e) = result {
                                error!("error" = &*e, "failed to apply EBL hooks");

                                let vfs_mounts = mem::take(&mut *vfs_mounts.lock().unwrap());

                                let guard = device_manager.push_vfs(&vfs_mounts);

                                mem::forget(guard);

                                return;
                            } else {
                                info!("kind" = "ebl", "applied asset override hook");
                            }
                        }
                        Err(e) => {
                            error!("BND4 snapshot error: {e}");
                            return;
                        }
                    }

                    if self.game != Game::DarkSouls3
                        && self.game != Game::EldenRing
                        && self.game != Game::Nightreign
                    {
                        info!("not a game with DLC, skipping hook");
                        return;
                    }

                    // 4.
                    let mount_dlc_ebl = unsafe { mount_dlc_ebl(self.image_base).map_err(|e| eyre!(e)) };

                    let result = mount_dlc_ebl.and_then(|mount_dlc_ebl| {
                        ModHost::get_attached()
                            .hook(mount_dlc_ebl)
                            .with_closure(move |p1, p2, p3, p4, trampoline| {
                                let mut device_manager = DlDeviceManagerLayout::lock(device_manager_ptr);

                                let snap = device_manager.snapshot();

                                unsafe {
                                    trampoline(p1, p2, p3, p4);
                                }

                                match snap {
                                    Ok(snap) => {
                                        let new = device_manager.extract_new(snap);

                                        if !new.is_empty() {
                                            debug!("extracted_mounts" = ?new);

                                            vfs_mounts.lock().unwrap().append(new);
                                        }
                                    }
                                    Err(e) => error!("BND4 snapshot error: {e}"),
                                }
                            })
                            .install().map_err(|e| eyre!(e))
                    });

                    if let Err(e) = result {
                        warn!("error" = &*e, "skipping DLC hook");
                    } else {
                        info!("kind" = "dlc", "applied asset override hook");
                    }
                })
            })
            .install()?;

        Ok(())
    }

    // Wwise must be hooked separately for games that have it.
    // The files are loaded through `DLMOW::IOHookBlocking`, which is a Wwise file location
    // resolver: https://www.audiokinetic.com/en/public-library/2024.1.6_8842/?source=SDK&id=class_a_k_1_1_stream_mgr_1_1_i_ak_file_location_resolver.html
    #[instrument(name = "wwise", skip_all)]
    fn try_hook_wwise(self: Arc<Self>) -> Result<(), eyre::Error> {
        if self.game < Game::EldenRing {
            info!("not a Wwise game, skipping hook");
            return Ok(());
        }

        let wwise_open_file = unsafe { find_wwise_open_file(self.image_base)? };

        ModHost::get_attached()
            .hook(wwise_open_file)
            .with_span(info_span!("hook"))
            .with_closure(move |p1, path, open_mode, p4, p5, p6, trampoline| {
                let path_string = unsafe { PCWSTR::from_raw(path).to_string().unwrap() };
                debug!("asset" = path_string);

                if let Some((mapped_path, mapped_override)) =
                    wwise::find_override(&self.mapping, &path_string)
                {
                    info!("override" = mapped_path);

                    unsafe {
                        trampoline(
                            p1,
                            mapped_override.as_ptr(),
                            AkOpenMode::Read as _,
                            p4,
                            p5,
                            p6,
                        )
                    }
                } else {
                    unsafe { trampoline(p1, path, open_mode, p4, p5, p6) }
                }
            })
            .install()?;

        info!("kind" = "wwise", "applied asset override hook");

        Ok(())
    }

    fn locate_device_manager(&self) -> DeviceManagerResult {
        self.device_manager
            .get_or_init(|| unsafe {
                match self.game {
                    Game::DarkSouls3 => {
                        DlDeviceManagerMsvc2012::find_device_manager(self.image_base)
                            .map(Either::Right)
                    }
                    _ => DlDeviceManager::find_device_manager(self.image_base).map(Either::Left),
                }
            })
            .clone()
    }

    fn enable_loose_params(&self) {
        // Some Dark Souls 3 mods use a legacy Mod Engine 2 option of loading "loose" param files
        // instead of Data0. For backwards compatibility me3 enables it below.
        if self.game != Game::DarkSouls3 {
            return;
        }

        const LOOSE_PARAM_FILES: [&str; 3] = [
            "data1:/param/gameparam/gameparam.parambnd.dcx",
            "data1:/param/gameparam/gameparam_dlc1.parambnd.dcx",
            "data1:/param/gameparam/gameparam_dlc2.parambnd.dcx",
        ];

        if LOOSE_PARAM_FILES
            .iter()
            .any(|file| self.mapping.vfs_override(file).is_some())
        {
            ModHost::get_attached()
                .override_game_property("Game.Debug.EnableRegulationFile", false);
        }
    }
}

type DeviceManagerResult = Result<
    Either<NonNull<DlDeviceManager>, NonNull<DlDeviceManagerMsvc2012>>,
    dl_device::FindError,
>;

struct AttachContext {
    game: Game,
    image_base: *const u8,
    mapping: Arc<ArchiveOverrideMapping>,
    device_manager: OnceLock<DeviceManagerResult>,
}

unsafe impl Send for AttachContext {}

unsafe impl Sync for AttachContext {}
