use std::{
    collections::HashMap,
    ffi::CString,
    fmt::Debug,
    marker::Tuple,
    panic::{self, AssertUnwindSafe},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

use closure_ffi::traits::FnPtr;
use eyre::eyre;
use libloading::{Library, Symbol};
use me3_launcher_attach_protocol::AttachConfig;
use me3_mod_protocol::{
    native::{Native, NativeInitializerCondition},
    profile::ModProfile,
    Game,
};
use retour::Function;
use tracing::{error, info, instrument, Span};

use self::hook::HookInstaller;
use crate::{
    detour::UntypedDetour,
    native::{ModEngineConnectorShim, ModEngineInitializer},
};

mod append;
pub mod game_properties;
pub mod hook;

static ATTACHED_INSTANCE: OnceLock<ModHost> = OnceLock::new();

#[derive(Default)]
pub struct ModHost {
    hooks: Mutex<Vec<Arc<UntypedDetour>>>,
    native_modules: Mutex<Vec<Library>>,
    profiles: Vec<ModProfile>,
    property_overrides: Mutex<HashMap<Vec<u16>, bool>>,
}

impl Debug for ModHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModHost")
            .field("hooks", &self.hooks)
            .field("profiles", &self.profiles)
            .field("property_overrides", &self.property_overrides)
            .finish()
    }
}

#[allow(unused)]
impl ModHost {
    pub fn new() -> Self {
        Self::default()
    }

    #[instrument(skip_all)]
    pub fn load_native(&self, native: &Native) -> eyre::Result<()> {
        let load_native = {
            let span = AssertUnwindSafe(Span::current());
            let native = native.clone();

            move || unsafe {
                let _span_guard = span.enter();
                let module = libloading::Library::new(&native.path)?;

                if let Some(NativeInitializerCondition {
                    function: Some(symbol),
                    ..
                }) = &native.initializer
                {
                    let sym_name = CString::new(symbol.as_bytes())?;

                    let initializer: Symbol<unsafe extern "C" fn() -> bool> =
                        module.get(sym_name.as_bytes_with_nul())?;

                    if !initializer() {
                        return Err(eyre!("native failed to initialize"));
                    }
                }

                info!("native" = native.name, "loaded native");

                eyre::Ok(module)
            }
        };

        let result = panic::catch_unwind(move || {
            if let Some(NativeInitializerCondition {
                delay: Some(delay), ..
            }) = &native.initializer
            {
                let name = native.name.clone();
                let delay = delay.clone();

                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(delay.ms as u64));

                    match load_native() {
                        Ok(module) => ModHost::get_attached()
                            .native_modules
                            .lock()
                            .unwrap()
                            .push(module),
                        Err(e) => {
                            error!(
                                "error" = &*e,
                                "native" = name,
                                "an error occurred while loading native"
                            )
                        }
                    }
                });

                return eyre::Ok(());
            }

            let module = load_native()?;

            if native.initializer.is_none() {
                let me2_initializer =
                    unsafe { module.get::<ModEngineInitializer>(b"modengine_ext_init\0") };

                if let Ok(initializer) = me2_initializer {
                    unsafe { initializer(&ModEngineConnectorShim, &mut std::ptr::null_mut()) };
                }
            }

            self.native_modules.lock().unwrap().push(module);

            eyre::Ok(())
        });

        match result {
            Ok(result) => result,
            Err(payload) => {
                let payload = payload
                    .downcast::<&'static str>()
                    .map_or("unable to retrieve panic payload", |b| *b);

                Err(eyre!(payload))
            }
        }
    }

    pub fn get_attached() -> &'static ModHost {
        ATTACHED_INSTANCE.get().expect("not attached")
    }

    pub fn attach(self) {
        ATTACHED_INSTANCE.set(self).expect("already attached");
    }

    pub fn hook<F>(&'static self, target: F) -> HookInstaller<F>
    where
        F: Function + FnPtr,
        F::Arguments: Tuple,
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

pub fn dearxan(attach_config: &AttachConfig) {
    if !attach_config.disable_arxan && attach_config.game != Game::DarkSouls3 {
        return;
    }

    info!(
        "game" = %attach_config.game,
        "attach_config.disable_arxan" = attach_config.disable_arxan,
        "will attempt to disable Arxan code protection",
    );

    let span = Span::current();
    unsafe {
        dearxan::disabler::neuter_arxan(move |result| {
            let _span_guard = span.enter();
            info!(?result, "dearxan::disabler::neuter_arxan finished");
        });
    }
}
