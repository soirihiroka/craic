pub fn is_permission_denied_message(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("permission denied") || message.contains("operation not permitted")
}
