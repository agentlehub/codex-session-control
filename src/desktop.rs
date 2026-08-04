mod descriptor;
mod discovery;
mod entry;

pub(crate) const DESKTOP_ENTRY_NAME: &str = "codex-desktop.desktop";
pub(crate) const DESKTOP_CAPABILITY: &str = "external-app-server-attachment-descriptor-v1";
pub(crate) const DESCRIPTOR_FILE_NAME: &str = "app-server-attachment.json";

#[allow(unused_imports)]
pub(crate) use descriptor::prepare_descriptor_parent;
pub(crate) use descriptor::{
    DescriptorState, inspect_descriptor, preflight_descriptor_switch, publish_descriptor,
    remove_expected_descriptor, render_descriptor,
};
#[allow(unused_imports)]
pub(crate) use discovery::{
    DesktopAvailability, DesktopLaunchCommand, DesktopTarget, discover_and_verify_desktop,
    inspect_desktop_availability, verify_persisted_desktop,
};

#[derive(Debug)]
enum DiscoveryFailure {
    Unavailable(String),
}

impl DiscoveryFailure {
    fn unavailable(reason: impl Into<String>) -> Self {
        Self::Unavailable(reason.into())
    }

    fn warning(self) -> String {
        match self {
            Self::Unavailable(reason) => format!("Desktop attachment unavailable: {reason}"),
        }
    }
}

#[cfg(test)]
mod tests;
