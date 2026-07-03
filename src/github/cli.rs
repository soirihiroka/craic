const AUTH_SWITCH_SCRIPT: &str = include_str!("scripts/auth_switch.sh");
const CHECKOUT_PULL_REQUEST_SCRIPT: &str = include_str!("scripts/checkout_pull_request.sh");
const GH_WITH_ACCOUNT_SCRIPT: &str = include_str!("scripts/gh_with_account.sh");

pub(crate) fn ssh_switch_auth_account_script(quoted_host: &str, quoted_login: &str) -> String {
    format!("set -- {quoted_host} {quoted_login}\n{AUTH_SWITCH_SCRIPT}")
}

pub(crate) fn ssh_checkout_pull_request_script(quoted_root: &str, quoted_number: &str) -> String {
    format!("set -- {quoted_root} {quoted_number}\n{CHECKOUT_PULL_REQUEST_SCRIPT}")
}

pub(crate) fn ssh_gh_with_account_script(host: &str, login: &str, args: &[String]) -> String {
    let mut command = format!("set -- {} {}", shell_quote(host), shell_quote(login));
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
