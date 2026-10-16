//! Shared, effect-free platform state directory selection.
pub fn platform_state_dir() -> Result<std::path::PathBuf, crate::workflow_history::LedgerError> {
    use crate::workflow_history::{LedgerError, LedgerUnavailableReason as R};
    if let Some(path) = std::env::var_os("A2A_BRIDGE_STATE_DIR") {
        if path.is_empty() {
            return Err(LedgerError::new(R::Open));
        }
        let path = std::path::PathBuf::from(path);
        if !path.is_absolute() {
            return Err(LedgerError::new(R::Open));
        }
        return Ok(path);
    }

    #[cfg(target_os = "macos")]
    let base = {
        let home = std::env::var_os("HOME")
            .filter(|v| !v.is_empty())
            .ok_or_else(|| LedgerError::new(R::Open))?;
        let home = std::path::PathBuf::from(home);
        if !home.is_absolute() {
            return Err(LedgerError::new(R::Open));
        }
        home.join("Library/Application Support/a2a-bridge")
    };
    #[cfg(not(target_os = "macos"))]
    let base = {
        let absolute_xdg = std::env::var_os("XDG_STATE_HOME")
            .filter(|value| !value.is_empty())
            .map(std::path::PathBuf::from)
            .filter(|path| path.is_absolute());
        if let Some(xdg) = absolute_xdg {
            xdg.join("a2a-bridge")
        } else {
            let home = std::env::var_os("HOME")
                .filter(|v| !v.is_empty())
                .ok_or_else(|| LedgerError::new(R::Open))?;
            let home = std::path::PathBuf::from(home);
            if !home.is_absolute() {
                return Err(LedgerError::new(R::Open));
            }
            home.join(".local/state").join("a2a-bridge")
        }
    };
    Ok(base)
}
