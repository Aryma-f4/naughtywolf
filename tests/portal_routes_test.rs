use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use naughtywolf::portal::{public_router, templates};
use tower::ServiceExt;

#[tokio::test]
async fn public_pages_link_local_styles_and_do_not_expose_operator_console_copy() {
    let app = public_router();
    let response = app
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();

    assert!(body.contains("/static/admin.css"));
    assert!(!body.to_lowercase().contains("sliver"));
    assert!(!body.to_lowercase().contains("payload"));
}

#[tokio::test]
async fn login_page_has_labeled_credentials_and_local_styles() {
    let response = public_router()
        .oneshot(Request::get("/login").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();

    assert!(body.contains("<label"));
    assert!(body.contains("autocomplete=\"username\""));
    assert!(body.contains("/static/admin.css"));
}

#[test]
fn invalid_login_template_is_generic_and_never_echoes_a_username() {
    let body = templates::login_page(Some("Invalid username or password"), "csrf-token");

    assert!(body.contains("Invalid username or password"));
    assert!(!body.contains("missing-user"));
}
