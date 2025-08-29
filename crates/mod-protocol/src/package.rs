use std::{
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::item::{AsItem, Item};

/// A package is a source for files that override files within the existing games DVDBND archives.
/// It points to a local path containing assets matching the hierarchy they would be served under in
/// the DVDBND.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct Package(pub(crate) Item);

impl Package {
    #[inline]
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        Item::new(path).into()
    }
}

impl AsRef<Path> for Package {
    #[inline]
    fn as_ref(&self) -> &Path {
        self.item().as_ref()
    }
}

impl Deref for Package {
    type Target = Item;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Package {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl AsItem for Package {
    #[inline]
    fn item(&self) -> &Item {
        &self.0
    }

    #[inline]
    fn item_mut(&mut self) -> &mut Item {
        &mut self.0
    }
}

impl From<Item> for Package {
    #[inline]
    fn from(item: Item) -> Self {
        Self(item)
    }
}

impl From<PathBuf> for Package {
    #[inline]
    fn from(path: PathBuf) -> Self {
        Item::from(path).into()
    }
}
