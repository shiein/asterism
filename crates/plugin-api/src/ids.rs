use asterism_core::action::ActionId;
use asterism_kernel::{KernelError, KernelManifest};

pub use asterism_kernel::TrustTier;

/// 命名空间 Action ID。旧 wire 名通过 `from_user` 映射。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ActionKey(pub &'static str);

impl ActionKey {
    pub const COPY: Self = Self("asterism.history.copy");
    pub const SAVE: Self = Self("asterism.history.save");
    pub const DELETE: Self = Self("asterism.history.delete");
    pub const FAVORITE: Self = Self("asterism.history.favorite");
    pub const SEND: Self = Self("asterism.sync.send_to_device");
    pub const CAPTURE_FULLSCREEN: Self = Self("asterism.capture.screenshot");
    pub const CAPTURE_REGION: Self = Self("asterism.capture.region");

    pub fn as_str(self) -> &'static str {
        self.0
    }

    pub fn from_legacy(id: ActionId) -> Self {
        match id {
            ActionId::Copy => Self::COPY,
            ActionId::Save => Self::SAVE,
            ActionId::Delete => Self::DELETE,
            ActionId::Favorite => Self::FAVORITE,
            ActionId::SendToDevice => Self::SEND,
        }
    }

    pub fn from_user(raw: &str) -> Result<Self, KernelError> {
        let mapped = match raw {
            "copy" => Self::COPY,
            "save" => Self::SAVE,
            "delete" => Self::DELETE,
            "favorite" => Self::FAVORITE,
            "send_to_device" => Self::SEND,
            other => {
                for key in [Self::COPY, Self::SAVE, Self::DELETE, Self::FAVORITE, Self::SEND] {
                    if key.0 == other {
                        return Ok(key);
                    }
                }
                return Err(KernelError::InvalidId(raw.to_string()));
            }
        };
        Ok(mapped)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PluginManifest {
    pub id: &'static str,
    pub trust_tier: TrustTier,
    pub requires: &'static [&'static str],
    pub permissions: &'static [&'static str],
}

impl PluginManifest {
    pub const fn kernel(self) -> KernelManifest {
        KernelManifest {
            id: self.id,
            requires: self.requires,
            permissions: self.permissions,
            trust_tier: self.trust_tier,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_legacy_wire_names() {
        assert_eq!(ActionKey::from_user("copy").unwrap(), ActionKey::COPY);
        assert_eq!(ActionKey::from_user("asterism.history.delete").unwrap(), ActionKey::DELETE);
    }
}
