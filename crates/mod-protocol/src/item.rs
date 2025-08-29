use std::{
    ops::BitXor,
    path::{Path, PathBuf},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub trait AsItem {
    fn item(&self) -> &Item;
    fn item_mut(&mut self) -> &mut Item;
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct Item {
    /// Name associated with this item.
    #[serde(rename = "$key$")]
    pub name: String,

    /// A path to the source of this item.
    #[serde(alias = "source")]
    pub path: PathBuf,

    /// Does this item participate in dependency resolution?
    #[serde(
        default = "Item::enabled_default",
        skip_serializing_if = "Item::enabled_skip_if_default"
    )]
    pub enabled: bool,

    /// Should failing to find this item result in a hard error?
    #[serde(
        default = "Item::optional_default",
        skip_serializing_if = "Item::optional_skip_if_default"
    )]
    pub optional: bool,
}

impl Item {
    #[inline]
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        path.as_ref().to_owned().into()
    }

    #[inline]
    pub fn is_relative(&self) -> bool {
        self.path.is_relative()
    }

    #[inline]
    pub fn make_absolute<P: AsRef<Path>>(&mut self, base: P) {
        if self.path.is_relative() {
            self.path = base.as_ref().join(&self.path);
        }
    }

    #[inline]
    fn enabled_default() -> bool {
        true
    }

    fn enabled_skip_if_default(enabled: &bool) -> bool {
        *enabled == Self::enabled_default()
    }

    #[inline]
    fn optional_default() -> bool {
        false
    }

    fn optional_skip_if_default(optional: &bool) -> bool {
        *optional == Self::optional_default()
    }
}

impl AsRef<Path> for Item {
    #[inline]
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl From<PathBuf> for Item {
    #[inline]
    fn from(path: PathBuf) -> Self {
        let fnv1_a = |b: &[u8]| {
            b.iter().fold(0x811c9dc5u32, |hash, byte| {
                hash.bitxor(*byte as u32).wrapping_mul(0x01000193)
            })
        };

        Self {
            name: format!(
                "{}-{:x}",
                path.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_lowercase(),
                fnv1_a(path.as_os_str().as_encoded_bytes())
            ),
            path,
            enabled: Self::enabled_default(),
            optional: Self::optional_default(),
        }
    }
}

impl AsItem for Item {
    #[inline]
    fn item(&self) -> &Item {
        self
    }

    #[inline]
    fn item_mut(&mut self) -> &mut Item {
        self
    }
}
