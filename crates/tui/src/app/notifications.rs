//! Desktop notification handling

/// Send desktop notification via notify-rust (if enabled in feature gates)
pub fn send_desktop_notification(title: &str, message: &str) {
    // Only send if desktop notifications are enabled
    if !crate::feature_gates().desktop_notify {
        return;
    }

    let title = title.to_string();
    let message = message.to_string();

    // Run in blocking task to avoid blocking async runtime
    tokio::task::spawn_blocking(move || {
        let _ = notify_rust::Notification::new()
            .summary(&title)
            .body(&message)
            .appname("Yomi")
            .timeout(notify_rust::Timeout::Milliseconds(5000))
            .show();
    });
}
