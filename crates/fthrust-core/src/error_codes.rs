//! Public registry of user-facing app failure codes.
//!
//! The app shows failures as `[Error NNN] TITLE` in its error bar. These codes
//! are a public registry, also published on the project's docs site, so the
//! strings here must not change without updating that documentation.

/// One documented app failure code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppFailureCode {
    /// Stable code, such as `"Error 001"`.
    pub code: &'static str,
    /// Bottom-bar title, unique across the registry.
    pub title: &'static str,
    /// Subsystem the failure belongs to.
    pub category: &'static str,
    /// Human-readable explanation of when the failure occurs.
    pub explanation: &'static str,
}

/// Every documented app failure code, in registry order.
pub const APP_FAILURE_CODES: [AppFailureCode; 23] = [
    AppFailureCode {
        code: "Error 001",
        title: "CAPTURE FAILED",
        category: "Capture",
        explanation: "Capture health reported a backend failure or the capture startup sequence failed. The detail text contains the reported reason.",
    },
    AppFailureCode {
        code: "Error 002",
        title: "CAPTURE UNAVAILABLE",
        category: "Capture",
        explanation: "A save was requested while the capture engine was not connected, so there was no active replay buffer to address.",
    },
    AppFailureCode {
        code: "Error 003",
        title: "ENGINE NOT FOUND",
        category: "Capture",
        explanation: "The FTHRcapture executable could not be found at the expected install or development path.",
    },
    AppFailureCode {
        code: "Error 004",
        title: "ENGINE NOT RESPONDING",
        category: "Capture",
        explanation: "The capture process was launched but did not complete its IPC startup handshake.",
    },
    AppFailureCode {
        code: "Error 005",
        title: "ENGINE COULD NOT START",
        category: "Capture",
        explanation: "A capture startup attempt failed before a usable engine connection was established.",
    },
    AppFailureCode {
        code: "Error 006",
        title: "ENGINE STOPPED",
        category: "Capture",
        explanation: "The capture process exited unexpectedly. The process exit code is included in the message detail.",
    },
    AppFailureCode {
        code: "Error 007",
        title: "MANUAL RECORDING FAILED",
        category: "Recording",
        explanation: "A manual recording could not be completed or finalized. Recoverable video fragments may be mentioned in the detail.",
    },
    AppFailureCode {
        code: "Error 008",
        title: "RECORDING COULD NOT START",
        category: "Recording",
        explanation: "The manual recording request could not be handed to the capture engine.",
    },
    AppFailureCode {
        code: "Error 009",
        title: "RECORDING COULD NOT STOP",
        category: "Recording",
        explanation: "The capture engine was unavailable when a manual recording stop was requested.",
    },
    AppFailureCode {
        code: "Error 010",
        title: "RECORDING FOLDER UNAVAILABLE",
        category: "Recording",
        explanation: "The configured manual-recording folder could not be created or accessed.",
    },
    AppFailureCode {
        code: "Error 011",
        title: "RECORDING PATH TOO LONG",
        category: "Recording",
        explanation: "The manual-recording output path exceeds the native engine path limit.",
    },
    AppFailureCode {
        code: "Error 012",
        title: "NOT ENOUGH DISK SPACE",
        category: "Recording",
        explanation: "Manual recording was blocked by the preflight free-space threshold.",
    },
    AppFailureCode {
        code: "Error 013",
        title: "CLIP NOT SAVED",
        category: "Clip saving",
        explanation: "Capture health rejected the save because fresh replay data was not currently usable. The detail says whether capture is starting, recovering, or stale.",
    },
    AppFailureCode {
        code: "Error 014",
        title: "CLIP SAVE FAILED",
        category: "Clip saving",
        explanation: "The local clip folder, save command submission, or save setup failed before a completed clip was produced.",
    },
    AppFailureCode {
        code: "Error 015",
        title: "CLIP WAS NOT SAVED",
        category: "Clip saving",
        explanation: "The capture engine reported a terminal save failure, an incomplete path, or a missing/empty output file.",
    },
    AppFailureCode {
        code: "Error 016",
        title: "CLIP PATH TOO LONG",
        category: "Clip saving",
        explanation: "The requested clip path exceeds the Windows shared-memory path field limit.",
    },
    AppFailureCode {
        code: "Error 017",
        title: "CLIP FINALIZATION FAILED",
        category: "Clip saving",
        explanation: "The base clip could not be made available as a completed final file after the save operation.",
    },
    AppFailureCode {
        code: "Error 018",
        title: "EXPORT FAILED",
        category: "Export and sharing",
        explanation: "An edited clip export failed validation, encoding, folder creation, or file publication.",
    },
    AppFailureCode {
        code: "Error 019",
        title: "SHARE FAILED",
        category: "Export and sharing",
        explanation: "The share workflow could not create or publish its exported clip.",
    },
    AppFailureCode {
        code: "Error 020",
        title: "UPLOAD FAILED",
        category: "Upload",
        explanation: "The upload provider rejected or could not complete the upload. The provider or retry detail is passed through when available.",
    },
    AppFailureCode {
        code: "Error 021",
        title: "UPLOAD NOT CONFIGURED",
        category: "Upload",
        explanation: "An upload was requested without an installed and enabled Upload Extension.",
    },
    AppFailureCode {
        code: "Error 022",
        title: "UPLOAD CONNECTION FAILED",
        category: "Upload",
        explanation: "The upload settings Test Connection request failed. The provider response is used as the detail.",
    },
    AppFailureCode {
        code: "Error 023",
        title: "COMPRESSION FAILED",
        category: "Upload",
        explanation: "The optional compressed upload copy could not be created, verified, or brought under the provider size limit.",
    },
];

/// Returns the registry entry whose title matches `title` exactly.
pub fn code_for_title(title: &str) -> Option<&'static AppFailureCode> {
    APP_FAILURE_CODES.iter().find(|code| code.title == title)
}

/// Formats a failure title as `[Error NNN] TITLE` when it is documented.
///
/// Unknown titles are returned unchanged.
pub fn format_error_title(title: &str) -> String {
    match code_for_title(title) {
        Some(code) => format!("[{}] {}", code.code, code.title),
        None => title.to_string(),
    }
}
