use crate::error::{KernelError, Result};

/// 校验 `asterism.clipboard` 这类点分标识，不认识领域语义。
pub fn validate_plugin_id(id: &str) -> Result<()> {
    let valid = id.split('.').all(|part| {
        let mut chars = part.chars();
        matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    }) && id.contains('.');
    if valid { Ok(()) } else { Err(KernelError::InvalidId(id.to_string())) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_namespaced_ids() {
        validate_plugin_id("asterism.clipboard").unwrap();
        validate_plugin_id("asterism.sync-core").unwrap();
        assert!(validate_plugin_id("Clipboard").is_err());
        assert!(validate_plugin_id("asterism").is_err());
    }
}
