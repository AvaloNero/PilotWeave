//! Default platform locations. Test substitution is absent from release builds.
use std::path::PathBuf;

pub fn config_dir() -> Option<PathBuf> {
    #[cfg(feature = "local-e2e")]
    if let Some(root) = crate::test_support::root() {
        return Some(root.join("config"));
    }
    dirs::config_dir()
}

pub fn home_dir() -> Option<PathBuf> {
    #[cfg(feature = "local-e2e")]
    if let Some(root) = crate::test_support::root() {
        return Some(root.join("home"));
    }
    dirs::home_dir()
}

pub fn http_config() -> ureq::config::ConfigBuilder<ureq::typestate::AgentScope> {
    let builder = ureq::Agent::config_builder();
    #[cfg(feature = "local-e2e")]
    if crate::test_support::active() {
        // Never forward synthetic authorization headers to an ambient proxy.
        return builder.https_only(false).proxy(None);
    }
    builder.https_only(true)
}

pub fn endpoint(url: &str) -> String {
    #[cfg(feature = "local-e2e")]
    if crate::test_support::active() {
        return crate::test_support::endpoint(url);
    }
    url.to_owned()
}
