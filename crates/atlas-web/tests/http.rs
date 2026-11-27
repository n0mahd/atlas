use std::fs;
use std::path::Path;

use atlas_fs::Sources;
use atlas_web::{Access, AppState, router};
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

const OWNER: &str = "owner@github";

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn app() -> (tempfile::TempDir, axum::Router) {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().to_path_buf();
    let k = home.join("Knowledge");
    write(&k.join("AGENTS.md"), "# Global rules\n");
    write(
        &k.join("skills/evil/SKILL.md"),
        "---\ndescription: \"<img src=x onerror=alert(1)>\"\n---\n# Evil\n<script>alert('x')</script>\n[click](javascript:alert(1))\n",
    );
    write(
        &k.join("memory/fact.md"),
        "---\nname: fact\ndescription: A fact\nmetadata:\n  type: user\n  host: all\n---\nSee [[fact]].\n",
    );
    let sources = Sources {
        home,
        knowledge: k,
        public_skills: None,
        projects: Vec::new(),
    };
    let app = router(
        AppState::new(sources, "test-hub", vec!["other-box".to_owned()]),
        Access::new(vec![OWNER.to_owned()]),
    );
    (tmp, app)
}

async fn get(
    app: &axum::Router,
    path: &str,
    login: Option<&str>,
) -> (StatusCode, axum::http::HeaderMap, String) {
    let mut req = Request::get(path);
    if let Some(login) = login {
        req = req.header("Tailscale-User-Login", login);
    }
    let res = app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let body = to_bytes(res.into_body(), 4 << 20).await.unwrap();
    (status, headers, String::from_utf8_lossy(&body).into_owned())
}

#[tokio::test]
async fn only_the_owner_gets_in() {
    let (_tmp, app) = app();
    assert_eq!(get(&app, "/", None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(
        get(&app, "/", Some("stranger@github")).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(&app, "/static/atlas.css", None).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(get(&app, "/", Some(OWNER)).await.0, StatusCode::OK);
}

#[tokio::test]
async fn every_page_renders_with_strict_headers() {
    let (_tmp, app) = app();
    for path in [
        "/",
        "/skills",
        "/skills?f=outside&q=ev",
        "/skills/private/evil",
        "/instructions",
        "/instructions/global/AGENTS.md",
        "/memory",
        "/memory?kind=user&host=shared",
        "/memory/fact",
        "/hooks",
        "/mcp",
        "/machines",
    ] {
        let (status, headers, body) = get(&app, path, Some(OWNER)).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        let csp = headers["content-security-policy"].to_str().unwrap();
        assert!(
            csp.contains("script-src 'self'") && !csp.contains("unsafe"),
            "{path}"
        );
        assert_eq!(headers["cache-control"], "no-store", "{path}");
        assert!(!body.contains(" style=\""), "inline style on {path}");
    }
}

#[tokio::test]
async fn hostile_content_cannot_script_the_page() {
    let (_tmp, app) = app();
    let (_, _, body) = get(&app, "/skills/private/evil", Some(OWNER)).await;
    assert!(body.contains("<h1>Evil</h1>"));
    // The only script tags are our own two files.
    let scripts: Vec<&str> = body
        .match_indices("<script")
        .map(|(i, _)| &body[i..i + 40])
        .collect();
    assert_eq!(scripts.len(), 2, "{scripts:?}");
    assert!(
        scripts
            .iter()
            .all(|t| t.starts_with("<script src=\"/static/")),
        "{scripts:?}"
    );
    // Escaped text may mention them; no tag or attribute may carry them.
    for bad in ["onerror=alert(1)>", "href=\"javascript", "<img src=x"] {
        assert!(!body.contains(bad), "{bad} reached the page");
    }
    let (_, _, list) = get(&app, "/skills", Some(OWNER)).await;
    assert!(
        list.contains("&#60;img src=x onerror=alert(1)&#62;"),
        "description must be escaped: {}",
        &list[list.find("entry-desc").unwrap_or(0)..][..200]
    );
}

#[tokio::test]
async fn unknown_paths_are_404() {
    let (_tmp, app) = app();
    for path in [
        "/skills/private/nope",
        "/memory/nope",
        "/static/../Cargo.toml",
        "/static/fonts/x.woff2",
        "/wat",
    ] {
        assert_eq!(
            get(&app, path, Some(OWNER)).await.0,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
}

#[tokio::test]
async fn wiki_links_and_search_work() {
    let (_tmp, app) = app();
    let (_, _, page) = get(&app, "/memory/fact", Some(OWNER)).await;
    assert!(page.contains(r#"<a href="/memory/fact""#));
    let (_, _, none) = get(&app, "/memory?q=zzz", Some(OWNER)).await;
    assert!(none.contains("No page matches."));
}
