use serde::Serialize;

#[derive(Debug, Serialize, thiserror::Error)]
#[error("[{code}] {message}")]
pub struct GuiError {
    pub code: &'static str,
    pub message: String,
}

impl GuiError {
    pub fn kernel(msg: impl ToString) -> Self {
        Self {
            code: "KERNEL_ERROR",
            message: msg.to_string(),
        }
    }

    pub fn not_connected() -> Self {
        Self {
            code: "NOT_CONNECTED",
            message: "Kernel daemon is not reachable.".into(),
        }
    }

    pub fn unknown(msg: impl ToString) -> Self {
        Self {
            code: "UNKNOWN",
            message: msg.to_string(),
        }
    }
}

impl From<anyhow::Error> for GuiError {
    fn from(e: anyhow::Error) -> Self {
        Self::kernel(e)
    }
}

impl From<std::io::Error> for GuiError {
    fn from(e: std::io::Error) -> Self {
        Self::unknown(e)
    }
}
