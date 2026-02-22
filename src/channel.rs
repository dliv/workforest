#[cfg(all(feature = "stable", feature = "beta"))]
compile_error!("features 'stable' and 'beta' are mutually exclusive");

#[cfg(feature = "stable")]
pub const APP_NAME: &str = "git-forest";
#[cfg(feature = "beta")]
pub const APP_NAME: &str = "git-forest-beta";

#[cfg(feature = "stable")]
pub const VERSION_CHANNEL: &str = "stable";
#[cfg(feature = "beta")]
pub const VERSION_CHANNEL: &str = "beta";

#[cfg(feature = "stable")]
pub const CMD: &str = "git forest";
#[cfg(feature = "beta")]
pub const CMD: &str = "git forest-beta";

pub const VERSION_CHECK_BASE_URL: &str = "https://forest.dliv.gg/api/latest";
pub const INTERNAL_VERSION_CHECK_ARG: &str = "--internal-version-check";
