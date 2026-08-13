use bitflags::bitflags;
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::id::{AccountId, DeviceId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DevicePlatform {
    Windows,
    Macos,
    LinuxHub,
    Browser,
}

impl DevicePlatform {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Macos => "macos",
            Self::LinuxHub => "linux_hub",
            Self::Browser => "browser",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "windows" => Ok(Self::Windows),
            "macos" => Ok(Self::Macos),
            "linux_hub" => Ok(Self::LinuxHub),
            "browser" => Ok(Self::Browser),
            other => Err(CoreError::InvalidPlatform(other.to_string())),
        }
    }

    pub fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Macos
        } else {
            Self::LinuxHub
        }
    }
}

bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
    #[serde(transparent)]
    pub struct DeviceCapabilities: u32 {
        const TEXT = 1 << 0;
        const IMAGE = 1 << 1;
        const FILE = 1 << 2;
        const SCREENSHOT = 1 << 3;
        const VIDEO = 1 << 4;
        const AUTO_RECEIVE = 1 << 5;
    }
}

impl DeviceCapabilities {
    pub fn desktop_v1() -> Self {
        Self::TEXT | Self::IMAGE | Self::FILE | Self::SCREENSHOT | Self::AUTO_RECEIVE
    }

    pub fn browser_v1() -> Self {
        Self::TEXT | Self::IMAGE | Self::FILE
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub id: DeviceId,
    pub account_id: AccountId,
    pub name: String,
    pub platform: DevicePlatform,
    pub identity_public_key: Vec<u8>,
    pub capabilities: DeviceCapabilities,
    pub created_at_ms: i64,
    pub last_seen_at_ms: i64,
    pub revoked_at_ms: Option<i64>,
}

impl Device {
    pub fn is_revoked(&self) -> bool {
        self.revoked_at_ms.is_some()
    }
}
