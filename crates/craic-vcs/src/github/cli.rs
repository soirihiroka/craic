const GH_WITH_ACCOUNT_SCRIPT: &str = include_str!("scripts/gh_with_account.sh");

pub fn gh_with_account_script(gh_path: &str, host: &str, login: &str, args: &[String]) -> String {
    let mut command = format!(
        "set -- {} {} {}",
        shell_quote(gh_path),
        shell_quote(host),
        shell_quote(login)
    );
    for arg in args {
        command.push(' ');
        command.push_str(&shell_quote(arg));
    }
    command.push('\n');
    command.push_str(GH_WITH_ACCOUNT_SCRIPT);
    command
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
