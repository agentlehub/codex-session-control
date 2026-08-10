mod descriptor;
mod discovery;
mod entry;

pub(crate) const DESKTOP_ENTRY_NAME: &str = "codex-desktop.desktop";
pub(crate) const DESKTOP_CAPABILITY: &str = "external-app-server-attachment-descriptor-v1";
pub(crate) use crate::model::DESKTOP_ATTACHMENT_DESCRIPTOR_FILE_NAME as DESCRIPTOR_FILE_NAME;

#[cfg(test)]
pub(crate) use descriptor::prepare_descriptor_parent;
pub(crate) use descriptor::{
    DescriptorPublicationFailure, DescriptorPublicationResidue, DescriptorState,
    inspect_descriptor, preflight_descriptor_switch, publish_descriptor,
    remove_expected_descriptor, render_descriptor,
};
pub(crate) use discovery::{
    DesktopAvailability, DesktopStructure, DesktopTarget, inspect_desktop_structure,
    probe_desktop_capability, probe_persisted_desktop_capability,
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
