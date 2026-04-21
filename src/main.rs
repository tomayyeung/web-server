use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    Router,
    extract::{Form, Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
};
use minijinja::{Environment, context};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tokio::sync::RwLock;

// Data model

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Bookmark {
    id: u64,
    url: String,
    title: String,
    tags: Vec<String>,
}

/// HTML form fields sent by the browser on POST /bookmarks.
#[derive(Debug, Deserialize)]
struct CreateBookmarkForm {
    url: String,
    title: String,
    tags: Option<String>,
}

#[derive(Default)]
struct BookmarkStore {
    next_id: u64,
    bookmarks: HashMap<u64, Bookmark>,
}

// Application state

/// Everything handlers need: the data store **and** the template engine.
///
/// We wrap the Environment in an Arc so it can be shared cheaply across
/// tasks.  It's immutable after setup, so no Mutex needed.
#[derive(Clone)]
struct AppState {
    // the bookmark store is now a database connection
    store: SqlitePool,
    templates: Arc<Environment<'static>>,
}

// Templating

/// Builds the MiniJinja environment with all our templates.
///
/// MiniJinja uses Jinja2 syntax:
///   {{ variable }}         -- output a value
///   {% for x in xs %}      -- control flow
///   {% block name %}       -- template inheritance
///
/// We define templates inline for simplicity.  In a real project you'd
/// load them from disk (Environment::set_loader).
fn build_templates() -> Environment<'static> {
    let mut env = Environment::new();

    env.add_template("base.html", include_str!("../templates/base.html"))
        .unwrap();

    env.add_template("list.html", include_str!("../templates/list.html"))
        .unwrap();

    env.add_template("detail.html", include_str!("../templates/detail.html"))
        .unwrap();

    env.add_template("new.html", include_str!("../templates/new.html"))
        .unwrap();

    env
}

/// Renders a template or returns a 500 error page.
///
/// Centralises the boilerplate of "get template → render → wrap in Html".
fn render(env: &Environment, name: &str, ctx: minijinja::Value) -> Response {
    match env.get_template(name).and_then(|t| t.render(ctx)) {
        Ok(html) => Html(html).into_response(),
        Err(e) => {
            eprintln!("template error: {e:#}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html("template error".to_string()),
            )
                .into_response()
        }
    }
}

// Handlers

// helper for list bookmarks so we can consolidate DB errors
async fn get_all_bookmarks(pool: &SqlitePool) -> sqlx::Result<Vec<Bookmark>> {
    let bookmarks =
        sqlx::query_as::<_, (u64, String, String)>("select id, url, title from bookmarks")
            .fetch_all(pool)
            .await?;

    let links = sqlx::query_as::<_, (u64, u64)>(r"SELECT bookmark_id, tag_id FROM bookmark_tag")
        .fetch_all(pool)
        .await?;

    let tags = sqlx::query_as::<_, (u64, String)>(
        r"SELECT id, name FROM tag WHERE id IN (SELECT tag_id FROM bookmark_tag)",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .collect::<HashMap<_, _>>();

    let mut tags_by_bookmark: HashMap<u64, Vec<String>> = HashMap::new();
    for (bookmark_id, tag_id) in &links {
        if let Some(name) = tags.get(tag_id) {
            tags_by_bookmark
                .entry(*bookmark_id)
                .or_default()
                .push(name.clone());
        }
    }

    let mut bookmarks: Vec<_> = bookmarks
        .into_iter()
        .map(|(id, url, title)| Bookmark {
            id,
            url,
            title,
            tags: tags_by_bookmark.remove(&id).unwrap_or_default(),
        })
        .collect();
    bookmarks.sort_by_key(|b| b.id);

    Ok(bookmarks)
}

/// GET /bookmarks
async fn list_bookmarks(State(state): State<AppState>) -> Response {
    let Ok(bookmarks) = get_all_bookmarks(&state.store).await else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Html("database error".to_string()),
        )
            .into_response();
    };
    render(&state.templates, "list.html", context! { bookmarks })
}

/// GET /bookmarks/new
///
/// Note: this route is registered **before** `/bookmarks/:id` so that the
/// literal path "new" isn't captured as an id.
async fn new_bookmark_form(State(state): State<AppState>) -> Response {
    render(&state.templates, "new.html", context! {})
}

/// GET /bookmarks/:id
async fn get_bookmark(State(state): State<AppState>, Path(id): Path<u64>) -> Response {
    let store = state.read_store().await;
    match store.bookmarks.get(&id) {
        Some(bm) => render(&state.templates, "detail.html", context! { bookmark => bm }),
        None => (
            StatusCode::NOT_FOUND,
            render(&state.templates, "404.html", context! {}),
        )
            .into_response(),
    }
}

/// POST /bookmarks
async fn create_bookmark(
    State(state): State<AppState>,
    Form(form): Form<CreateBookmarkForm>,
) -> Redirect {
    let tags: Vec<String> = form
        .tags
        .unwrap_or_default()
        .split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();

    let id = {
        let mut store = state.store.write().await;
        let id = store.next_id;
        store.next_id += 1;
        store.bookmarks.insert(
            id,
            Bookmark {
                id,
                url: form.url,
                title: form.title,
                tags,
            },
        );
        id
    };

    Redirect::to(&format!("/bookmarks/{id}"))
}

fn build_router(state: AppState) -> Router {
    // Important: `/bookmarks/new` must be registered before `/bookmarks/:id`
    // so that "new" isn't interpreted as an id parameter.
    Router::new()
        .route("/bookmarks", get(list_bookmarks).post(create_bookmark))
        .route("/bookmarks/new", get(new_bookmark_form))
        .route("/bookmarks/{id}", get(get_bookmark))
        .with_state(state)
}

// Main

#[tokio::main]
async fn main() {
    let state = AppState {
        store: Arc::new(RwLock::new(BookmarkStore::default())),
        templates: Arc::new(build_templates()),
    };

    // A background task that logs the store size every 30 seconds.
    let bg = state.store.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            let store = bg.read().await;
            println!("[background] {} bookmark(s)", store.bookmarks.len());
        }
    });

    let app = build_router(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:8080")
        .await
        .expect("failed to bind port 8080");

    println!("Open http://127.0.0.1:8080/bookmarks in your browser");
    axum::serve(listener, app).await.expect("server error");
}
