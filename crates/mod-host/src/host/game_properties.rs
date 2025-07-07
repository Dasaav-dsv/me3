use std::{collections::HashMap, mem};

use either::Either;
use eyre::OptionExt;
use me3_mod_host_assets::{
    pe,
    string::{DlUtf16String, DlUtf16StringMsvc2012},
};
use me3_mod_protocol::Game;
use regex::bytes::Regex;
use tracing::{debug, error, instrument, Span};

use crate::{deferred::defer_until_init, host::ModHost};

type GetBoolProperty<S> = unsafe extern "C" fn(usize, *const S, bool) -> bool;

impl ModHost {
    #[instrument(skip_all)]
    pub fn override_game_properties(&'static self) -> Result<(), eyre::Error> {
        if self.game < Game::ArmoredCore6 {
            self.override_before_ac6()
        } else {
            Ok(())
        }
    }

    fn override_before_ac6(&'static self) -> Result<(), eyre::Error> {
        let get_bool_property = if self.game == Game::DarkSouls3 {
            Either::Left(self.bool_property_getter::<DlUtf16StringMsvc2012>()?)
        } else {
            Either::Right(self.bool_property_getter::<DlUtf16String>()?)
        };

        either::for_both!(get_bool_property, get_bool_property => {
            debug!(?get_bool_property);

            // Some games (Dark Souls 3) might employ Arxan encryption
            // that is removed after running the Arxan entrypoint.
            defer_until_init(Span::current(), move || {
                let result = self
                    .hook(get_bool_property)
                    .with_closure(|p1, name, default, trampoline| {
                        let property = unsafe { name.as_ref().unwrap().get().unwrap() };

                        let state = self
                            .property_overrides
                            .lock()
                            .unwrap()
                            .get(property.as_bytes())
                            .copied()
                            .unwrap_or_else(|| unsafe { trampoline(p1, name, default) });

                        debug!(%property, state);

                        state
                    })
                    .install();

                if let Err(e) = result {
                    error!("error" = %e, "failed to hook property getter");
                }
            })?;
        });

        Ok(())
    }

    fn bool_property_getter<S>(&'static self) -> Result<GetBoolProperty<S>, eyre::Error> {
        // Dark Souls 3 uses the same pattern, except for the different allocator layout in
        // containers.
        let function_call_re = if self.game == Game::DarkSouls3 {
            Regex::new(
                r"(?s-u)(?:\x48\x8d\x54\x24\x30\x48\x8b\x0d....\xe8(....)\x88\x05....\x48\x83\x7c\x24\x48\x08\x72.)|(?:\x48\x8d\x54\x24\x30\x48\x8b\x0d....\xe8(....)\x0f\xb6\xd8\x48\x83\x7c\x24\x48\x08\x72.)",
            )
        } else {
            Regex::new(
                r"(?s-u)(?:\x48\x8d\x54\x24\x30\x48\x8b\x0d....\xe8(....)\x88\x05....\x48\x83\x7c\x24\x50\x08\x72.)|(?:\x48\x8d\x54\x24\x30\x48\x8b\x0d....\xe8(....)\x0f\xb6\xd8\x48\x83\x7c\x24\x50\x08\x72.)",
            )
        }?;

        let [text] = unsafe { pe::sections(self.image_base(), [".text"])? };

        let all_calls = function_call_re
            .captures_iter(text)
            .map(|c| {
                let (_, [call_disp32]) = c.extract();
                let call_bytes = <[u8; 4]>::try_from(call_disp32).unwrap();

                call_disp32
                    .as_ptr_range()
                    .end
                    .wrapping_byte_offset(i32::from_le_bytes(call_bytes) as _)
            })
            .fold(HashMap::<_, usize>::new(), |mut map, ptr| {
                *map.entry(ptr).or_default() += 1;
                map
            });

        all_calls
            .into_iter()
            .fold(None, |largest, (ptr, count)| {
                let (largest, largest_count) = largest.unwrap_or_default();

                if largest_count < count {
                    Some((ptr, count))
                } else {
                    Some((largest, largest_count))
                }
            })
            .map(|(ptr, _)| unsafe { mem::transmute::<_, GetBoolProperty<S>>(ptr) })
            .ok_or_eyre("pattern returned no matches")
    }
}
