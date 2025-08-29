use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, KeyValueMap};

use crate::{
    item::Item,
    native::{Native, NativeInitializerCondition},
    package::Package,
    Game,
};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(from = "ModProfileV2Layout", into = "ModProfileV2Layout")]
pub struct ModProfileV2 {
    /// The game that this profile supports.
    pub supports: Option<Game>,

    /// Native modules (DLLs) that will be loaded.
    pub natives: Vec<Native>,

    /// A collection of packages containing assets to be added to the virtual file system.
    pub packages: Vec<Package>,

    /// Other profiles listed as dependencies by this profile.
    pub profiles: Vec<Item>,

    /// Name of an alternative savefile to use (in the default savefile directory).
    pub savefile: Option<String>,

    /// Starts the game with multiplayer server connectivity enabled.
    pub start_online: Option<bool>,

    /// Try to neutralize Arxan GuardIT code protection to improve mod stability.
    pub disable_arxan: Option<bool>,
}

impl ModProfileV2 {
    pub(super) fn push_dependency(&mut self, uses: ProfileDependency) {
        let name = uses
            .inner
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();

        if name.ends_with(".dll") {
            self.natives.push(uses.into());
        } else if name.ends_with(".me3")
            || name.ends_with(".me3.toml")
            || name.ends_with(".me3.json")
        {
            self.profiles.push(uses.into());
        } else {
            self.packages.push(uses.into());
        }
    }
}

#[serde_as]
#[derive(Default, Deserialize, Serialize, JsonSchema)]
struct ModProfileV2Layout {
    #[serde(default)]
    game: ProfileGame,

    #[serde_as(as = "KeyValueMap<_>")]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    dependencies: Vec<ProfileDependency>,
}

#[derive(Default, Deserialize, Serialize, JsonSchema)]
struct ProfileGame {
    #[serde(default)]
    launch: Option<Game>,

    #[serde(default)]
    savefile: Option<String>,

    #[serde(default)]
    start_online: Option<bool>,

    #[serde(default)]
    disable_arxan: Option<bool>,
}

#[derive(Clone, Deserialize, Serialize, JsonSchema)]
pub struct ProfileDependency {
    #[serde(flatten)]
    inner: Item,

    #[serde(default)]
    initializer: Option<NativeInitializerCondition>,
}

impl From<ModProfileV2Layout> for ModProfileV2 {
    fn from(layout: ModProfileV2Layout) -> Self {
        let mut profile = Self {
            supports: layout.game.launch,
            savefile: layout.game.savefile,
            start_online: layout.game.start_online,
            disable_arxan: layout.game.disable_arxan,
            ..Default::default()
        };

        for dep in layout.dependencies {
            profile.push_dependency(dep);
        }

        profile
    }
}

impl From<ModProfileV2> for ModProfileV2Layout {
    fn from(profile: ModProfileV2) -> Self {
        let mut dependencies = vec![];

        dependencies.extend(profile.natives.into_iter().map(Into::into));
        dependencies.extend(profile.packages.into_iter().map(Into::into));
        dependencies.extend(profile.profiles.into_iter().map(Into::into));

        Self {
            game: ProfileGame {
                launch: profile.supports,
                savefile: profile.savefile,
                start_online: profile.start_online,
                disable_arxan: profile.disable_arxan,
            },
            dependencies,
        }
    }
}

impl From<ProfileDependency> for Native {
    fn from(uses: ProfileDependency) -> Self {
        Self {
            initializer: uses.initializer,
            ..uses.inner.into()
        }
    }
}

impl From<Native> for ProfileDependency {
    fn from(native: Native) -> Self {
        Self {
            inner: native.inner,
            initializer: native.initializer,
        }
    }
}

impl From<ProfileDependency> for Package {
    fn from(uses: ProfileDependency) -> Self {
        uses.inner.into()
    }
}

impl From<Package> for ProfileDependency {
    fn from(package: Package) -> Self {
        package.0.into()
    }
}

impl From<ProfileDependency> for Item {
    fn from(uses: ProfileDependency) -> Self {
        uses.inner
    }
}

impl From<Item> for ProfileDependency {
    fn from(item: Item) -> Self {
        Self {
            inner: item,
            initializer: None,
        }
    }
}

impl JsonSchema for ModProfileV2 {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ModProfileV2".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schema_for!(ModProfileV2Layout)
    }
}
