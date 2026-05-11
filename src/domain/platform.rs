#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Linux,
}

impl Platform {
    pub fn short_name(self) -> &'static str {
        match self {
            Platform::MacOs => "macos",
            Platform::Linux => "linux",
        }
    }
}

pub fn current_platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::MacOs
    } else {
        Platform::Linux
    }
}
