pub fn public_landing() -> String {
    page_shell(
        "NaughtyWolf",
        "<main class=\"public-page\"><p class=\"eyebrow\">Local security lab</p><h1>Practice with clear scope.</h1><p>Manage authorized operations, inventory, checks, evidence, and audit records in one local workspace.</p><a class=\"button\" href=\"/login\">Sign in</a></main>",
    )
}

pub fn login_page(error: Option<&str>, csrf_token: &str) -> String {
    let error = error
        .map(|message| {
            format!(
                "<p class=\"form-error\" role=\"alert\">{}</p>",
                escape_html(message)
            )
        })
        .unwrap_or_default();

    page_shell(
        "Sign in",
        &format!(
            "<main class=\"login-page\"><form method=\"post\" action=\"/login\"><h1>Sign in</h1>{error}<input type=\"hidden\" name=\"csrf_token\" value=\"{}\"><label for=\"username\">Username</label><input id=\"username\" name=\"username\" autocomplete=\"username\" required><label for=\"password\">Password</label><input id=\"password\" name=\"password\" type=\"password\" autocomplete=\"current-password\" required><button type=\"submit\">Sign in</button></form></main>",
            escape_html(csrf_token)
        ),
    )
}

fn page_shell(title: &str, content: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{} · NaughtyWolf</title><link rel=\"stylesheet\" href=\"/static/admin.css\"><script src=\"/static/admin.js\" defer></script></head><body>{content}</body></html>",
        escape_html(title),
    )
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}
