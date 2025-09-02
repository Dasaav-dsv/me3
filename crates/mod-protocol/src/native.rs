use std::{
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::item::{AsItem, Item};

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct NativeInitializerDelay {
    pub ms: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct NativeInitializerCondition {
    #[serde(default)]
    pub delay: Option<NativeInitializerDelay>,
    #[serde(default)]
    pub function: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Native {
    #[serde(flatten)]
    pub(crate) inner: Item,

    /// An optional symbol to be called after this native successfully loads.
    pub initializer: Option<NativeInitializerCondition>,
}

impl Native {
    #[inline]
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        Item::new(path).into()
    }

    #[inline]
    pub fn is_default(&self) -> bool {
        self.inner.is_default() && self.initializer.is_none()
    }
}

impl AsRef<Path> for Native {
    #[inline]
    fn as_ref(&self) -> &Path {
        self.item().as_ref()
    }
}

impl Deref for Native {
    type Target = Item;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for Native {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl AsItem for Native {
    #[inline]
    fn item(&self) -> &Item {
        &self.inner
    }

    #[inline]
    fn item_mut(&mut self) -> &mut Item {
        &mut self.inner
    }
}

impl From<Item> for Native {
    #[inline]
    fn from(item: Item) -> Self {
        Self {
            inner: item,
            initializer: None,
        }
    }
}

impl From<PathBuf> for Native {
    #[inline]
    fn from(path: PathBuf) -> Self {
        Item::from(path).into()
    }
}
