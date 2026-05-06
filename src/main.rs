use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    Router,
    extract::{Form, Path, State, FromRequestParts},
    http::{StatusCode, request::Parts},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    middleware::{self, Next},
};
use minijinja::{Environment, context};
use password_auth::verify_password;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use uuid::Uuid;
use std::sync::RwLock;

// ==================== Data Models ====================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct User {
    pub id: String,
    pub username: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Bookmark {
    id: u64,
    url: String,
    title: String,
    tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CreateBookmarkForm {
    url: String,
    title: String,
    tags: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LoginForm {
    username: String,
    password: String,
}

#[derive(Debug, Deserialize)]
struct RegisterForm {
    username: String,
    password: String,
    password_confirm: String,
}

// ==================== Application State ====================

#[derive(Clone)]
struct AppState {
    store: SqlitePool,
    templates: Arc<Environment<'static>>,
    sessions: Arc<RwLock<HashMap<String, String>>>, // session_id -> user_id
}

// ==================== Error Responses ====================

fn database_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Html("database error".to_string()),
    )
        .into_response()
}

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

// ==================== Templates ====================

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

    env.add_template("modify.html", include_str!("../templates/modify.html"))
        .unwrap();

    env.add_template("login.html", include_str!("../templates/login.html"))
        .unwrap();

    env.add_template("register.html", include_str!("../templates/register.html"))
        .unwrap();

    env
}

// ==================== Database Queries ====================

async fn find_user_by_username(pool: &SqlitePool, username: &str) -> sqlx::Result<Option<User>> {
    sqlx::query_as::<_, (String, String)>(
        "SELECT id, username FROM users WHERE username = ?",
    )
    .bind(username)
    .fetch_optional(pool)
    .await
    .map(|opt| opt.map(|(id, username)| User { id, username }))
}

#[allow(dead_code)]
async fn find_user_by_id(pool: &SqlitePool, id: &str) -> sqlx::Result<Option<User>> {
    sqlx::query_as::<_, (String, String)>(
        "SELECT id, username FROM users WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map(|opt| opt.map(|(id, username)| User { id, username }))
}

async fn find_user_password_hash(pool: &SqlitePool, username: &str) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar::<_, String>(
        "SELECT password_hash FROM users WHERE username = ?",
    )
    .bind(username)
    .fetch_optional(pool)
    .await
}

async fn user_exists(pool: &SqlitePool, username: &str) -> sqlx::Result<bool> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM users WHERE username = ?)",
    )
    .bind(username)
    .fetch_one(pool)
    .await
}

async fn create_user(
    pool: &SqlitePool,
    username: &str,
    password_hash: &str,
) -> sqlx::Result<()> {
    let id = Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO users (id, username, password_hash) VALUES (?, ?, ?)")
        .bind(id)
        .bind(username)
        .bind(password_hash)
        .execute(pool)
        .await?;
    Ok(())
}

async fn get_user_bookmarks(pool: &SqlitePool, user_id: &str) -> sqlx::Result<Vec<Bookmark>> {
    let bookmarks = sqlx::query_as::<_, (u64, String, String)>(
        "SELECT id, url, title FROM bookmark WHERE user_id = ? ORDER BY id",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    let links = sqlx::query_as::<_, (u64, u64)>(
        "SELECT bookmark_id, tag_id FROM bookmark_tag WHERE bookmark_id IN (SELECT id FROM bookmark WHERE user_id = ?)",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    let tags = sqlx::query_as::<_, (u64, String)>(
        "SELECT id, name FROM tag WHERE id IN (SELECT tag_id FROM bookmark_tag)",
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

    let bookmarks = bookmarks
        .into_iter()
        .map(|(id, url, title)| Bookmark {
            id,
            url,
            title,
            tags: tags_by_bookmark.remove(&id).unwrap_or_default(),
        })
        .collect();

    Ok(bookmarks)
}

async fn get_user_bookmark(
    pool: &SqlitePool,
    user_id: &str,
    bookmark_id: u64,
) -> sqlx::Result<Option<Bookmark>> {
    let Some((id, url, title)) = sqlx::query_as::<_, (u64, String, String)>(
        "SELECT id, url, title FROM bookmark WHERE user_id = ? AND id = ?",
    )
    .bind(user_id)
    .bind(bookmark_id as i64)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };

    let tags = sqlx::query_scalar::<_, String>(
        "SELECT tag.name FROM tag, bookmark_tag bt WHERE tag.id = bt.tag_id AND bt.bookmark_id = ?",
    )
    .bind(bookmark_id as i64)
    .fetch_all(pool)
    .await?;

    Ok(Some(Bookmark {
        id,
        url,
        title,
        tags,
    }))
}

async fn create_bookmark_for_user(
    pool: &SqlitePool,
    user_id: &str,
    url: String,
    title: String,
    tags: Vec<String>,
) -> sqlx::Result<i64> {
    let mut trans = pool.begin().await?;

    let bookmark_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO bookmark (user_id, url, title) VALUES (?, ?, ?) RETURNING id",
    )
    .bind(user_id)
    .bind(url)
    .bind(title)
    .fetch_one(&mut *trans)
    .await?;

    // create the tags
    if !tags.is_empty() {
        let placeholders = vec!["(?)"; tags.len()].join(", ");
        let query_text = format!(r"INSERT OR IGNORE INTO tag (name) VALUES {placeholders}");
        let insert_query = tags
            .iter()
            .fold(sqlx::query(&query_text), |query, tag| query.bind(tag));
        insert_query.execute(&mut *trans).await?;

        // create the links
        let placeholders = vec!["?"; tags.len()].join(", ");
        let link_tags = format!(
            r"INSERT INTO bookmark_tag (bookmark_id, tag_id)
                  SELECT ?, id FROM tag WHERE name IN ({placeholders})"
        );
        let mut q = sqlx::query(&link_tags).bind(bookmark_id);
        for tag in tags {
            q = q.bind(tag);
        }
        q.execute(&mut *trans).await?;
    }

    trans.commit().await?;

    Ok(bookmark_id)
}

async fn delete_bookmark_if_owner(
    pool: &SqlitePool,
    user_id: &str,
    bookmark_id: u64,
) -> sqlx::Result<bool> {
    let mut trans = pool.begin().await?;

    // Check if user owns this bookmark
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM bookmark WHERE id = ? AND user_id = ?)",
    )
    .bind(bookmark_id as i64)
    .bind(user_id)
    .fetch_one(&mut *trans)
    .await?;

    if !exists {
        return Ok(false);
    }

    sqlx::query("DELETE FROM bookmark_tag WHERE bookmark_id = ?")
        .bind(bookmark_id as i64)
        .execute(&mut *trans)
        .await?;

    sqlx::query("DELETE FROM bookmark WHERE id = ?")
        .bind(bookmark_id as i64)
        .execute(&mut *trans)
        .await?;

    trans.commit().await?;

    Ok(true)
}

async fn update_bookmark_if_owner(
    pool: &SqlitePool,
    user_id: &str,
    bookmark_id: u64,
    url: String,
    title: String,
    tags: Vec<String>,
) -> sqlx::Result<bool> {
    let mut trans = pool.begin().await?;

    // Check if user owns this bookmark
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM bookmark WHERE id = ? AND user_id = ?)",
    )
    .bind(bookmark_id as i64)
    .bind(user_id)
    .fetch_one(&mut *trans)
    .await?;

    if !exists {
        return Ok(false);
    }

    // Update url and title
    sqlx::query("UPDATE bookmark SET url = ?, title = ? WHERE id = ?")
        .bind(&url)
        .bind(title)
        .bind(bookmark_id as i64)
        .execute(&mut *trans)
        .await?;

    // clear tags
    sqlx::query("DELETE FROM bookmark_tag WHERE bookmark_id = ?")
        .bind(bookmark_id as i64)
        .execute(&mut *trans)
        .await?;

    // recreate the tags
    if !tags.is_empty() {
        let placeholders = vec!["(?)"; tags.len()].join(", ");
        let query_text = format!(r"INSERT OR IGNORE INTO tag (name) VALUES {placeholders}");
        let insert_query = tags
            .iter()
            .fold(sqlx::query(&query_text), |query, tag| query.bind(tag));
        insert_query.execute(&mut *trans).await?;

        // recreate the links
        let placeholders = vec!["?"; tags.len()].join(", ");
        let link_tags = format!(
            r"INSERT INTO bookmark_tag (bookmark_id, tag_id)
                  SELECT ?, id FROM tag WHERE name IN ({placeholders})"
        );
        let mut q = sqlx::query(&link_tags).bind(bookmark_id as i64);
        for tag in tags {
            q = q.bind(tag);
        }
        q.execute(&mut *trans).await?;
    }

    trans.commit().await?;

    Ok(true)
}

pub struct LoggedInUser(pub User);

#[async_trait]
impl<S> FromRequestParts<S> for LoggedInUser
where
    S: Send + Sync,
{
    type Rejection = Redirect;

    fn from_request_parts(parts: &mut Parts, _state: &S) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> {
        async {
            let user = parts
                .extensions
                .get::<User>()
                .cloned()
                .ok_or_else(|| Redirect::to("/login"))?;

            Ok(LoggedInUser(user))
        }
    }
}

// ==================== Handlers: Authentication ====================

/// GET /login
async fn login_page(State(state): State<AppState>) -> Response {
    render(&state.templates, "login.html", context! {})
}

/// POST /login
async fn handle_login(
    State(state): State<AppState>,
    Form(form): Form<LoginForm>,
) -> Response {
    // Find user by username
    let user = match find_user_by_username(&state.store, &form.username).await {
        Ok(Some(u)) => u,
        _ => {
            return render(
                &state.templates,
                "login.html",
                context! { error => "Invalid username or password" },
            );
        }
    };

    // Get password hash
    let hash = match find_user_password_hash(&state.store, &form.username).await {
        Ok(Some(h)) => h,
        _ => {
            return render(
                &state.templates,
                "login.html",
                context! { error => "Invalid username or password" },
            );
        }
    };

    // Verify password
    let password_bytes = form.password.as_bytes();
    if verify_password(password_bytes, &hash).is_err() {
        return render(
            &state.templates,
            "login.html",
            context! { error => "Invalid username or password" },
        );
    }

    // Create session
    let session_id = Uuid::new_v4().to_string();
    {
        let mut sessions = state.sessions.write().unwrap();
        sessions.insert(session_id.clone(), user.id.clone());
    }

    // Redirect with session cookie
    let mut response = Redirect::to("/bookmarks").into_response();
    if let Ok(header_value) = format!("session_id={}; Path=/", session_id).parse() {
        response.headers_mut().insert("Set-Cookie", header_value);
    }
    response
}

/// GET /register
async fn register_page(State(state): State<AppState>) -> Response {
    render(&state.templates, "register.html", context! {})
}

/// POST /register
async fn handle_register(
    State(state): State<AppState>,
    Form(form): Form<RegisterForm>,
) -> Response {
    // Validate passwords match
    if form.password != form.password_confirm {
        return render(
            &state.templates,
            "register.html",
            context! { error => "Passwords do not match" },
        );
    }

    // Validate password length
    if form.password.len() < 8 {
        return render(
            &state.templates,
            "register.html",
            context! { error => "Password must be at least 8 characters" },
        );
    }

    // Check if username already exists
    match user_exists(&state.store, &form.username).await {
        Ok(true) => {
            return render(
                &state.templates,
                "register.html",
                context! { error => "Username already exists" },
            );
        }
        Err(_) => return database_error(),
        _ => {}
    }

    // Hash password
    let password_hash = password_auth::generate_hash(&form.password);

    // Create user
    if let Err(_) = create_user(&state.store, &form.username, &password_hash).await {
        return database_error();
    }

    // Redirect to login
    Redirect::to("/login").into_response()
}

/// POST /logout
async fn handle_logout(State(_state): State<AppState>) -> Response {
    // Clear session by removing the cookie
    let mut response = Redirect::to("/login").into_response();
    if let Ok(header_value) = "session_id=; Path=/; Max-Age=0".parse() {
        response.headers_mut().insert("Set-Cookie", header_value);
    }
    response
}

// ==================== Handlers: Bookmarks ====================

/// GET /bookmarks
async fn list_bookmarks(
    State(state): State<AppState>,
    LoggedInUser(user): LoggedInUser,
) -> Response {
    match get_user_bookmarks(&state.store, &user.id).await {
        Ok(bookmarks) => render(&state.templates, "list.html", context! { bookmarks, username => user.username }),
        Err(_) => database_error(),
    }
}

/// GET /bookmarks/new
async fn new_bookmark_form(State(state): State<AppState>, LoggedInUser(_): LoggedInUser) -> Response {
    render(&state.templates, "new.html", context! {})
}

/// GET /bookmarks/:id
async fn get_bookmark(
    State(state): State<AppState>,
    LoggedInUser(user): LoggedInUser,
    Path(id): Path<u64>,
) -> Response {
    match get_user_bookmark(&state.store, &user.id, id).await {
        Err(_) => database_error(),
        Ok(Some(bm)) => render(&state.templates, "detail.html", context! { bookmark => bm, username => user.username }),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Html("Bookmark not found"),
        )
            .into_response(),
    }
}

/// POST /bookmarks
async fn create_bookmark(
    State(state): State<AppState>,
    LoggedInUser(user): LoggedInUser,
    Form(form): Form<CreateBookmarkForm>,
) -> Response {
    let tags: Vec<String> = form
        .tags
        .unwrap_or_default()
        .split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();

    match create_bookmark_for_user(&state.store, &user.id, form.url, form.title, tags).await {
        Ok(id) => Redirect::to(&format!("/bookmarks/{id}")).into_response(),
        Err(_) => database_error(),
    }
}

/// GET /modify/:id
async fn modify_page(
    State(state): State<AppState>,
    LoggedInUser(user): LoggedInUser,
    Path(id): Path<u64>,
) -> Response {
    match get_user_bookmark(&state.store, &user.id, id).await {
        Err(_) => database_error(),
        Ok(Some(bm)) => render(&state.templates, "modify.html", context! { bookmark => bm, username => user.username }),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Html("Bookmark not found"),
        )
            .into_response(),
    }
}

/// POST /modify/:id
async fn modify_bookmark(
    State(state): State<AppState>,
    LoggedInUser(user): LoggedInUser,
    Path(id): Path<u64>,
    Form(form): Form<CreateBookmarkForm>,
) -> Response {
    let tags: Vec<String> = form
        .tags
        .unwrap_or_default()
        .split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();

    match update_bookmark_if_owner(&state.store, &user.id, id, form.url, form.title, tags).await {
        Ok(true) => Redirect::to(&format!("/bookmarks/{id}")).into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Html("Bookmark not found"),
        )
            .into_response(),
        Err(_) => database_error(),
    }
}

/// POST /bookmarks/:id (delete)
async fn delete_bookmark(
    State(state): State<AppState>,
    LoggedInUser(user): LoggedInUser,
    Path(id): Path<u64>,
) -> Response {
    match delete_bookmark_if_owner(&state.store, &user.id, id).await {
        Ok(true) => Redirect::to("/bookmarks").into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Html("Bookmark not found"),
        )
            .into_response(),
        Err(_) => database_error(),
    }
}

// ==================== Router ====================

fn build_router(state: AppState) -> Router {
    let state_for_router = state.clone();
    let session_layer = middleware::from_fn(move |mut req: axum::http::Request<axum::body::Body>, next: Next| {
        let state = state_for_router.clone();
        async move {
            // Try to get session ID from cookie
            if let Some(cookie_header) = req.headers().get("cookie") {
                if let Ok(cookie_str) = cookie_header.to_str() {
                    // Parse session_id from cookie
                    for part in cookie_str.split(';') {
                        if let Some(value) = part.trim().strip_prefix("session_id=") {
                            // Look up user in sessions
                            let user_id_opt = {
                                if let Ok(sessions) = state.sessions.read() {
                                    sessions.get(value).cloned()
                                } else {
                                    None
                                }
                            };

                            if let Some(user_id) = user_id_opt {
                                // Look up user from database
                                match find_user_by_id(&state.store, &user_id).await {
                                    Ok(Some(user)) => {
                                        req.extensions_mut().insert(user);
                                    }
                                    _ => {}
                                }
                            }
                            break;
                        }
                    }
                }
            }

            next.run(req).await
        }
    });

    Router::new()
        // Auth routes (public)
        .route("/login", get(login_page).post(handle_login))
        .route("/register", get(register_page).post(handle_register))
        .route("/logout", post(handle_logout))
        // Bookmark routes (require login)
        .route("/bookmarks", get(list_bookmarks).post(create_bookmark))
        .route("/bookmarks/new", get(new_bookmark_form))
        .route("/bookmarks/{id}", get(get_bookmark).post(delete_bookmark))
        .route("/modify/{id}", get(modify_page).post(modify_bookmark))
        .with_state(state)
        .layer(session_layer)
}

// ==================== Main ====================

#[tokio::main]
async fn main() {
    let pool = SqlitePool::connect("sqlite:bookmarks.db?mode=rwc")
        .await
        .expect("Cannot connect to the database");
    sqlx::raw_sql(include_str!("../schema.sql"))
        .execute(&pool)
        .await
        .expect("Cannot create the schema");
    sqlx::raw_sql(include_str!("../fixtures.sql"))
        .execute(&pool)
        .await
        .expect("Cannot load fixtures");

    let state = AppState {
        store: pool,
        templates: Arc::new(build_templates()),
        sessions: Arc::new(RwLock::new(HashMap::new())),
    };

    let app = build_router(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:8080")
        .await
        .expect("failed to bind port 8080");

    println!("Open http://127.0.0.1:8080/login in your browser");
    axum::serve(listener, app).await.expect("server error");
}
