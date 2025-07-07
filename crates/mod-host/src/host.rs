use std::{
    collections::HashMap,
    ffi::CString,
    fmt::Debug,
    panic,
    path::Path,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

use closure_ffi::traits::FnPtr;
use libloading::{Library, Symbol};
use me3_mod_protocol::{native::NativeInitializerCondition, Game};
use retour::Function;
use tracing::{error, info, warn};
use windows::{core::PCWSTR, Win32::System::LibraryLoader::GetModuleHandleW};

use self::hook::HookInstaller;
use crate::{
    detour::UntypedDetour,
    native::{ModEngineConnectorShim, ModEngineExtension, ModEngineInitializer},
};

mod append;
mod game_properties;
pub mod hook;

static ATTACHED_INSTANCE: OnceLock<ModHost> = OnceLock::new();

pub struct ModHost {
    game: Game,
    image_base: usize,
    hooks: Mutex<Vec<Arc<UntypedDetour>>>,
    native_modules: Mutex<Vec<Library>>,
    property_overrides: Mutex<HashMap<Vec<u16>, bool>>,
}

impl Debug for ModHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModHost")
            .field("game", &self.game)
            .field("hooks", &self.hooks)
            .finish()
    }
}

#[allow(unused)]
impl ModHost {
    pub fn attach(game: Game) {
        let image_base = unsafe { GetModuleHandleW(PCWSTR::null()) }
            .expect("GetModuleHandleW failed")
            .0 as usize;

        let instance = Self {
            game,
            image_base,
            hooks: Default::default(),
            native_modules: Default::default(),
            property_overrides: Default::default(),
        };

        ATTACHED_INSTANCE.set(instance).expect("already attached");

        if let Err(e) = Self::get_attached().override_game_properties() {
            error!("error" = &*e, "failed to attach game property override");
        }
    }

    pub fn get_attached() -> &'static ModHost {
        ATTACHED_INSTANCE.get().expect("not attached")
    }

    pub fn game(&self) -> Game {
        self.game
    }

    pub fn image_base(&self) -> *const u8 {
        self.image_base as _
    }

    pub fn load_native(
        &self,
        path: &Path,
        condition: Option<NativeInitializerCondition>,
    ) -> eyre::Result<()> {
        let result = panic::catch_unwind(|| {
            let module = unsafe { libloading::Library::new(path)? };

            match &condition {
                Some(NativeInitializerCondition::Delay { ms }) => {
                    std::thread::sleep(Duration::from_millis(*ms as u64))
                }
                Some(NativeInitializerCondition::Function(symbol)) => unsafe {
                    let sym_name = CString::new(symbol.as_bytes())?;
                    let initializer: Symbol<unsafe extern "C" fn() -> bool> =
                        module.get(sym_name.as_bytes_with_nul())?;

                    if initializer() {
                        info!(?path, symbol, "native initialized successfully");
                    } else {
                        error!(?path, symbol, "native failed to initialize");
                    }
                },
                None => {
                    let me2_initializer: Option<Symbol<ModEngineInitializer>> =
                        unsafe { module.get(b"modengine_ext_init\0").ok() };

                    let mut extension_ptr: *mut ModEngineExtension = std::ptr::null_mut();
                    if let Some(initializer) = me2_initializer {
                        unsafe { initializer(&ModEngineConnectorShim, &mut extension_ptr) };

                        info!(?path, "loaded native with me2 compatibility shim");
                    }
                }
            }

            Ok(module)
        });

        match result {
            Err(exception) => {
                warn!("an error occurred while loading {path:?}, it may not work as expected");
                Ok(())
            }
            Ok(result) => result.map(|module| {
                self.native_modules.lock().unwrap().push(module);
            }),
        }
    }

    pub fn hook<F>(&'static self, target: F) -> HookInstaller<F>
    where
        F: Function + FnPtr,
    {
        HookInstaller::new(target).on_install(|hook| self.hooks.lock().unwrap().push(hook))
    }

    pub fn override_game_property<S: AsRef<str>>(&self, property: S, state: bool) {
        self.property_overrides
            .lock()
            .unwrap()
            .insert(property.as_ref().encode_utf16().collect(), state);
    }
}
