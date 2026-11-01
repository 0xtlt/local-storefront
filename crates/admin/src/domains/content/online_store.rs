//! The online store's content: `pageCreate`, `pageUpdate`, `pageDelete`, `blogCreate`,
//! `blogUpdate`, `blogDelete`, `articleCreate`, `articleUpdate`, `articleDelete`, and the
//! moderation of comments (`commentApprove`, `commentSpam`, `commentNotSpam`, `commentDelete`).
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - A handle "will be generated automatically from the page title" (the article's, the blog's)
//!   when there is none, with `-1`, `-2` when it is taken (an article's within its blog). A
//!   handle given that is taken is refused (`TAKEN`).
//! - A page is visible (`isPublished`) by default "if no publish date is specified"; a
//!   `publishDate` publishes it then (in the future, it is not published yet); `isPublished: true`
//!   with a future `publishDate` is refused (`INVALID_PUBLISH_DATE`). Articles follow the same
//!   rules.
//! - `articleCreate` takes its blog by `blogId`, or creates one from `blog: {title}`; not both
//!   (`AMBIGUOUS_BLOG`), one of them (`BLOG_REFERENCE_REQUIRED`). Its author is a `name` (an
//!   `userId` names a staff member, which lsf does not have).
//! - `redirectNewHandle` creates the URL redirect from the old path to the new one, as the admin
//!   does (`/pages/old` → `/pages/new`); `redirectArticles` of a blog does for its articles too.
//! - `metafields` are set as `metafieldsSet` sets them.
//! - A comment approved is `PUBLISHED`, marked as spam `SPAM`, not spam `UNAPPROVED` ("restoring
//!   it to normal moderation status"); deleted, it is gone.
//!
//! Messages are Rails' validations ("Title can't be blank", "Handle has already been taken"),
//! "Can’t set isPublished to true and also set a future publish date." as the admin words it,
//! the descriptions of the codes otherwise.
//!
//! Events: Shopify has no webhook topic for pages, blogs, articles or comments in 2026-10: their
//! changes are the events `pages/create`, `blogs/update`, `articles/delete`, `comments/update`, …,
//! which no subscription can name (`tagsAdd` emits `articles/update` the same way).

use serde_json::{Map, Value, json};

use super::metafields::{self, check_owned_inputs, set_owned, text};
use super::navigation::add_redirect;
use crate::domains::catalog::{gids, tags_of};
use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::state::{Record, ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("pageCreate", page_create)
        .mutation("pageUpdate", page_update)
        .mutation("pageDelete", page_delete)
        .mutation("blogCreate", blog_create)
        .mutation("blogUpdate", blog_update)
        .mutation("blogDelete", blog_delete)
        .mutation("articleCreate", article_create)
        .mutation("articleUpdate", article_update)
        .mutation("articleDelete", article_delete)
        .mutation("commentApprove", |m, ctx| moderate(m, ctx, "PUBLISHED"))
        .mutation("commentSpam", |m, ctx| moderate(m, ctx, "SPAM"))
        .mutation("commentNotSpam", |m, ctx| moderate(m, ctx, "UNAPPROVED"))
        .mutation("commentDelete", comment_delete);
}

/// The longest title Shopify takes.
const TITLE_LENGTH: usize = 255;

/// The checks of a title: required on creation, at most 255 characters.
fn check_title(
    input: &Map<String, Value>,
    path: &str,
    required: bool,
    errors: &mut Vec<UserError>,
) {
    match input.get("title").and_then(Value::as_str) {
        Some(title) if title.trim().is_empty() => {
            errors.push(UserError::new(&[path, "title"], "Title can't be blank").code("BLANK"))
        }
        None if required => {
            errors.push(UserError::new(&[path, "title"], "Title can't be blank").code("BLANK"))
        }
        Some(title) if title.chars().count() > TITLE_LENGTH => errors.push(
            UserError::new(
                &[path, "title"],
                "Title is too long (maximum is 255 characters)",
            )
            .code("TOO_LONG"),
        ),
        _ => {}
    }
}

/// The handle of a creation or an update: the one given (handleized; refused when taken), else
/// for a creation one made from the title, free.
fn handle_for(
    state: &ShopState,
    type_name: &str,
    input: &Map<String, Value>,
    path: &str,
    scope: Option<(&str, &str)>,
    except: Option<&str>,
    errors: &mut Vec<UserError>,
) -> Option<String> {
    if let Some(given) = input.get("handle").and_then(Value::as_str) {
        let handle = lsf_core::util::handleize(given);
        if handle.is_empty() {
            errors.push(UserError::new(&[path, "handle"], "Handle can't be blank").code("BLANK"));
        } else if state.handle_taken(type_name, &handle, scope, except) {
            errors.push(
                UserError::new(&[path, "handle"], "Handle has already been taken").code("TAKEN"),
            );
        }
        return Some(handle);
    }
    if except.is_some() {
        return None;
    }
    let title = input
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let base = match lsf_core::util::handleize(title) {
        base if base.is_empty() => type_name.to_ascii_lowercase(),
        base => base,
    };
    Some(crate::state::free_handle(&base, |handle| {
        state.handle_taken(type_name, handle, scope, None)
    }))
}

/// When a page or an article is published: `(isPublished, publishedAt)` from `isPublished` and
/// `publishDate`, `before` the record's when it is an update. A future date with
/// `isPublished: true` is refused.
fn publication(
    input: &Map<String, Value>,
    now: &str,
    before: Option<&Record>,
    path: &str,
    errors: &mut Vec<UserError>,
) -> (bool, Value) {
    let date = input.get("publishDate").and_then(Value::as_str);
    let flag = input.get("isPublished").and_then(Value::as_bool);
    let future = date.is_some_and(|date| {
        let parse = |text: &str| chrono::DateTime::parse_from_rfc3339(text).ok();
        matches!((parse(date), parse(now)), (Some(date), Some(now)) if date > now)
    });
    if flag == Some(true) && future {
        errors.push(
            UserError::new(
                &[path, "publishDate"],
                "Can’t set isPublished to true and also set a future publish date.",
            )
            .code("INVALID_PUBLISH_DATE"),
        );
    }
    let was = before.map(|record| {
        (
            record
                .get("isPublished")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            record.get("publishedAt").cloned().unwrap_or(Value::Null),
        )
    });
    match (flag, date) {
        (Some(false), _) => (false, Value::Null),
        (_, Some(date)) => (!future, json!(date)),
        (Some(true), None) => match was {
            Some((true, at)) if !at.is_null() => (true, at),
            _ => (true, json!(now)),
        },
        (None, None) => was.unwrap_or((true, json!(now))),
    }
}

/// The template suffix of an input: `""` or `null` is the default template.
fn template_suffix(input: &Map<String, Value>) -> Option<Value> {
    input
        .get("templateSuffix")
        .map(|suffix| json!(suffix.as_str().filter(|suffix| !suffix.is_empty())))
}

fn remove_metafields(state: &mut ShopState, record: &Record) {
    for metafield in gids(record.get("metafields")) {
        metafields::remove(state, &metafield);
    }
}

// ---- pages -------------------------------------------------------------------------------

fn page_create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx.arg_object_or_default("page");
    let state = mutation.state();
    let now = state.timestamp();
    let mut errors = Vec::new();
    check_title(&input, "page", true, &mut errors);
    let handle = handle_for(state, "Page", &input, "page", None, None, &mut errors);
    let (published, published_at) = publication(&input, &now, None, "page", &mut errors);
    let owned = check_owned_inputs(
        state,
        None,
        "PAGE",
        input.get("metafields"),
        &["page", "metafields"],
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(coded(
            json!({"page": null}),
            &errors,
            "PageCreateUserErrorCode",
        ));
    }
    let state = mutation.state_mut();
    let id = state.new_gid("Page");
    let body = input
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or_default();
    state.insert(value::object(json!({
        "__typename": "Page",
        "id": id,
        "title": input.get("title").and_then(Value::as_str).unwrap_or_default().trim(),
        "handle": handle,
        "body": body,
        "bodySummary": summary(body),
        "isPublished": published,
        "publishedAt": published_at,
        "templateSuffix": template_suffix(&input).unwrap_or(Value::Null),
        "createdAt": now,
        "updatedAt": now,
        "metafields": [],
    })));
    set_owned(state, &id, &owned);
    state.emit("pages/create", &id);
    Ok(payload(json!({"page": id}), &[]))
}

/// The text of a body, as `bodySummary`: its first 150 characters (lsf's choice: Shopify does not
/// say how long a summary is).
fn summary(body: &str) -> String {
    value::plain_text(body).chars().take(150).collect()
}

fn page_update(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let input = ctx.arg_object_or_default("page");
    let state = mutation.state();
    let Some(record) = (state.type_of(&id) == Some("Page"))
        .then(|| state.get(&id).cloned())
        .flatten()
    else {
        let error = UserError::new(&["id"], "Page does not exist").code("NOT_FOUND");
        return Ok(payload(json!({"page": null}), &[error]));
    };
    let now = state.timestamp();
    let mut errors = Vec::new();
    check_title(&input, "page", false, &mut errors);
    let handle = handle_for(state, "Page", &input, "page", None, Some(&id), &mut errors);
    let (published, published_at) = publication(&input, &now, Some(&record), "page", &mut errors);
    let owned = check_owned_inputs(
        state,
        Some(&id),
        "PAGE",
        input.get("metafields"),
        &["page", "metafields"],
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(coded(
            json!({"page": null}),
            &errors,
            "PageUpdateUserErrorCode",
        ));
    }
    let old_handle = text(&record, "handle").to_string();
    let state = mutation.state_mut();
    if let Some(page) = state.get_mut(&id) {
        if let Some(title) = input.get("title").and_then(Value::as_str) {
            page.insert("title".into(), json!(title.trim()));
        }
        if let Some(body) = input.get("body").and_then(Value::as_str) {
            page.insert("body".into(), json!(body));
            page.insert("bodySummary".into(), json!(summary(body)));
        }
        if let Some(handle) = &handle {
            page.insert("handle".into(), json!(handle));
        }
        if let Some(suffix) = template_suffix(&input) {
            page.insert("templateSuffix".into(), suffix);
        }
        page.insert("isPublished".into(), json!(published));
        page.insert("publishedAt".into(), published_at);
    }
    set_owned(state, &id, &owned);
    if let Some(handle) = handle.filter(|handle| *handle != old_handle)
        && input.get("redirectNewHandle").and_then(Value::as_bool) == Some(true)
    {
        add_redirect(
            state,
            &format!("/pages/{old_handle}"),
            &format!("/pages/{handle}"),
        );
    }
    state.touch(&id);
    state.emit("pages/update", &id);
    Ok(payload(json!({"page": id}), &[]))
}

fn page_delete(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let Some(record) = (mutation.state().type_of(&id) == Some("Page"))
        .then(|| mutation.state().get(&id).cloned())
        .flatten()
    else {
        let error = UserError::new(&["id"], "Page does not exist").code("NOT_FOUND");
        return Ok(payload(json!({"deletedPageId": null}), &[error]));
    };
    let state = mutation.state_mut();
    state.delete(&id, "pages/delete");
    remove_metafields(state, &record);
    Ok(payload(json!({"deletedPageId": id}), &[]))
}

// ---- blogs -------------------------------------------------------------------------------

/// Inserts a blog: what `blogCreate` and `articleCreate`'s `blog: {title}` make.
fn insert_blog(state: &mut ShopState, input: &Map<String, Value>, handle: &str) -> String {
    let now = state.timestamp();
    let id = state.new_gid("Blog");
    state.insert(value::object(json!({
        "__typename": "Blog",
        "id": id,
        "title": input.get("title").and_then(Value::as_str).unwrap_or_default().trim(),
        "handle": handle,
        // A blog's comments are off until the merchant turns them on (the admin's default).
        "commentPolicy": input.get("commentPolicy").and_then(Value::as_str).unwrap_or("CLOSED"),
        "articles": [],
        "tags": [],
        "templateSuffix": template_suffix(input).unwrap_or(Value::Null),
        "feed": null,
        "createdAt": now,
        "updatedAt": now,
        "metafields": [],
    })));
    state.emit("blogs/create", &id);
    id
}

fn blog_create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx.arg_object_or_default("blog");
    let state = mutation.state();
    let mut errors = Vec::new();
    check_title(&input, "blog", true, &mut errors);
    let handle = handle_for(state, "Blog", &input, "blog", None, None, &mut errors);
    let owned = check_owned_inputs(
        state,
        None,
        "BLOG",
        input.get("metafields"),
        &["blog", "metafields"],
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(coded(
            json!({"blog": null}),
            &errors,
            "BlogCreateUserErrorCode",
        ));
    }
    let state = mutation.state_mut();
    let id = insert_blog(state, &input, &handle.unwrap_or_default());
    set_owned(state, &id, &owned);
    Ok(payload(json!({"blog": id}), &[]))
}

fn blog_update(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let input = ctx.arg_object_or_default("blog");
    let state = mutation.state();
    let Some(record) = (state.type_of(&id) == Some("Blog"))
        .then(|| state.get(&id).cloned())
        .flatten()
    else {
        let error = UserError::new(&["id"], "Blog does not exist").code("NOT_FOUND");
        return Ok(coded(
            json!({"blog": null}),
            &[error],
            "BlogUpdateUserErrorCode",
        ));
    };
    let mut errors = Vec::new();
    check_title(&input, "blog", false, &mut errors);
    let handle = handle_for(state, "Blog", &input, "blog", None, Some(&id), &mut errors);
    let owned = check_owned_inputs(
        state,
        Some(&id),
        "BLOG",
        input.get("metafields"),
        &["blog", "metafields"],
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(coded(
            json!({"blog": null}),
            &errors,
            "BlogUpdateUserErrorCode",
        ));
    }
    let old_handle = text(&record, "handle").to_string();
    let state = mutation.state_mut();
    if let Some(blog) = state.get_mut(&id) {
        if let Some(title) = input.get("title").and_then(Value::as_str) {
            blog.insert("title".into(), json!(title.trim()));
        }
        if let Some(policy) = input.get("commentPolicy").and_then(Value::as_str) {
            blog.insert("commentPolicy".into(), json!(policy));
        }
        if let Some(handle) = &handle {
            blog.insert("handle".into(), json!(handle));
        }
        if let Some(suffix) = template_suffix(&input) {
            blog.insert("templateSuffix".into(), suffix);
        }
    }
    set_owned(state, &id, &owned);
    if let Some(handle) = handle.filter(|handle| *handle != old_handle) {
        let redirect = |flag: &str| input.get(flag).and_then(Value::as_bool) == Some(true);
        if redirect("redirectNewHandle") {
            add_redirect(
                state,
                &format!("/blogs/{old_handle}"),
                &format!("/blogs/{handle}"),
            );
        }
        if redirect("redirectArticles") {
            for article in gids(record.get("articles")) {
                let article_handle = state
                    .get(&article)
                    .map(|article| text(article, "handle").to_string())
                    .unwrap_or_default();
                add_redirect(
                    state,
                    &format!("/blogs/{old_handle}/{article_handle}"),
                    &format!("/blogs/{handle}/{article_handle}"),
                );
            }
        }
    }
    state.touch(&id);
    state.emit("blogs/update", &id);
    Ok(payload(json!({"blog": id}), &[]))
}

/// `blogDelete`: the blog, its articles and their comments, and their metafields.
fn blog_delete(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let Some(record) = (mutation.state().type_of(&id) == Some("Blog"))
        .then(|| mutation.state().get(&id).cloned())
        .flatten()
    else {
        let error = UserError::new(&["id"], "Blog does not exist").code("NOT_FOUND");
        return Ok(coded(
            json!({"deletedBlogId": null}),
            &[error],
            "BlogDeleteUserErrorCode",
        ));
    };
    let state = mutation.state_mut();
    for article in gids(record.get("articles")) {
        remove_article(state, &article);
    }
    state.delete(&id, "blogs/delete");
    remove_metafields(state, &record);
    Ok(payload(json!({"deletedBlogId": id}), &[]))
}

/// The tags of a blog: those of its articles, in order.
pub fn refresh_blog(state: &mut ShopState, blog: &str) {
    let articles = state
        .get(blog)
        .map(|record| gids(record.get("articles")))
        .unwrap_or_default();
    let mut tags: Vec<String> = Vec::new();
    for article in &articles {
        for tag in state
            .get(article)
            .map(|record| tags_of(record.get("tags")))
            .unwrap_or_default()
        {
            if !tags.contains(&tag) {
                tags.push(tag);
            }
        }
    }
    if let Some(record) = state.get_mut(blog) {
        record.insert("tags".into(), json!(tags));
    }
}

// ---- articles ----------------------------------------------------------------------------

/// The blog of an `articleCreate` or `articleUpdate`: `blogId` (an existing blog), or `blog:
/// {title}` (one to create, `Err` its title). `None` when neither is given.
fn blog_of(
    state: &ShopState,
    input: &Map<String, Value>,
    new_blog: Option<&Value>,
    errors: &mut Vec<UserError>,
) -> Option<Result<String, String>> {
    let id = input.get("blogId").and_then(Value::as_str);
    let title = new_blog
        .and_then(|blog| blog.get("title"))
        .and_then(Value::as_str);
    match (id, title) {
        (Some(_), Some(_)) => {
            errors.push(
                UserError::new(
                    &["article"],
                    "Can't create a blog from input if a blog ID is supplied.",
                )
                .code("AMBIGUOUS_BLOG"),
            );
            None
        }
        (Some(id), None) => {
            if state.type_of(id) != Some("Blog") {
                errors.push(
                    UserError::new(&["article", "blogId"], "Blog does not exist").code("NOT_FOUND"),
                );
                return None;
            }
            Some(Ok(id.to_string()))
        }
        (None, Some(title)) => {
            if title.trim().is_empty() {
                errors
                    .push(UserError::new(&["blog", "title"], "Title can't be blank").code("BLANK"));
            }
            Some(Err(title.to_string()))
        }
        (None, None) => None,
    }
}

/// The author of an input: a name. A staff member (`userId`) is not one lsf has.
fn author_of(
    input: &Map<String, Value>,
    required: bool,
    errors: &mut Vec<UserError>,
) -> Option<Value> {
    let author = input.get("author").filter(|author| !author.is_null());
    let name = author
        .and_then(|author| author.get("name"))
        .and_then(Value::as_str);
    let user = author
        .and_then(|author| author.get("userId"))
        .and_then(Value::as_str);
    match (name, user) {
        (Some(_), Some(_)) => {
            errors.push(
                UserError::new(
                    &["article", "author"],
                    "Can't create an article author if both author name and user ID are supplied.",
                )
                .code("AMBIGUOUS_AUTHOR"),
            );
            None
        }
        (Some(name), None) if !name.trim().is_empty() => Some(json!({"name": name.trim()})),
        (None, Some(_)) => {
            errors.push(
                UserError::new(
                    &["article", "author", "userId"],
                    "User must exist if a user ID is supplied.",
                )
                .code("AUTHOR_MUST_EXIST"),
            );
            None
        }
        _ if required || author.is_some() => {
            errors.push(
                UserError::new(
                    &["article", "author"],
                    "Author name or user ID is required.",
                )
                .code("AUTHOR_FIELD_REQUIRED"),
            );
            None
        }
        _ => None,
    }
}

/// The image of an article: `{url, altText}`, an `Image` with its id.
fn image_of(state: &mut ShopState, input: &Map<String, Value>) -> Option<Value> {
    let image = input.get("image").filter(|image| !image.is_null())?;
    let url = image.get("url").and_then(Value::as_str)?;
    let id = state.new_gid("ArticleImage");
    Some(json!({
        "id": id,
        "url": url,
        "altText": image.get("altText").cloned().unwrap_or(Value::Null),
        "width": null,
        "height": null,
    }))
}

fn article_create(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let input = ctx.arg_object_or_default("article");
    let state = mutation.state();
    let now = state.timestamp();
    let mut errors = Vec::new();
    check_title(&input, "article", true, &mut errors);
    let blog = blog_of(state, &input, ctx.arg("blog"), &mut errors);
    if blog.is_none() && errors.is_empty() {
        errors.push(
            UserError::new(
                &["article"],
                "Must reference or create a blog when creating an article.",
            )
            .code("BLOG_REFERENCE_REQUIRED"),
        );
    }
    let author = author_of(&input, true, &mut errors);
    let scope = match &blog {
        Some(Ok(blog)) => Some(blog.as_str()),
        _ => None,
    };
    let handle = handle_for(
        state,
        "Article",
        &input,
        "article",
        Some(("blog", scope.unwrap_or("-"))),
        None,
        &mut errors,
    );
    let (published, published_at) = publication(&input, &now, None, "article", &mut errors);
    let owned = check_owned_inputs(
        state,
        None,
        "ARTICLE",
        input.get("metafields"),
        &["article", "metafields"],
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(coded(
            json!({"article": null}),
            &errors,
            "ArticleCreateUserErrorCode",
        ));
    }
    let state = mutation.state_mut();
    let blog = match blog {
        Some(Ok(blog)) => blog,
        Some(Err(title)) => {
            let mut blog_input = Map::new();
            blog_input.insert("title".into(), json!(title));
            let handle = handle_for(
                state,
                "Blog",
                &blog_input,
                "blog",
                None,
                None,
                &mut Vec::new(),
            )
            .unwrap_or_default();
            insert_blog(state, &blog_input, &handle)
        }
        None => return Ok(payload(json!({"article": null}), &[])),
    };
    let id = state.new_gid("Article");
    let image = image_of(state, &input);
    state.insert(value::object(json!({
        "__typename": "Article",
        "id": id,
        "title": input.get("title").and_then(Value::as_str).unwrap_or_default().trim(),
        "handle": handle,
        "blog": blog,
        "author": author,
        "body": input.get("body").and_then(Value::as_str).unwrap_or_default(),
        "summary": input.get("summary").cloned().unwrap_or(Value::Null),
        "image": image,
        "tags": tags_of(input.get("tags")),
        "isPublished": published,
        "publishedAt": published_at,
        "templateSuffix": template_suffix(&input).unwrap_or(Value::Null),
        "comments": [],
        "createdAt": now,
        "updatedAt": now,
        "metafields": [],
    })));
    if let Some(Value::Array(articles)) = state
        .get_mut(&blog)
        .and_then(|record| record.get_mut("articles"))
    {
        articles.push(json!(id));
    }
    refresh_blog(state, &blog);
    set_owned(state, &id, &owned);
    state.emit("articles/create", &id);
    Ok(payload(json!({"article": id}), &[]))
}

/// `articleUpdate`: a `blogId` (or `blog: {title}`) moves the article to that blog.
fn article_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let input = ctx.arg_object_or_default("article");
    let state = mutation.state();
    let Some(record) = (state.type_of(&id) == Some("Article"))
        .then(|| state.get(&id).cloned())
        .flatten()
    else {
        let error = UserError::new(&["id"], "Article does not exist").code("NOT_FOUND");
        return Ok(payload(json!({"article": null}), &[error]));
    };
    let now = state.timestamp();
    let mut errors = Vec::new();
    check_title(&input, "article", false, &mut errors);
    let blog = blog_of(state, &input, ctx.arg("blog"), &mut errors);
    let author = author_of(&input, false, &mut errors);
    let current_blog = text(&record, "blog").to_string();
    let scope = match &blog {
        Some(Ok(blog)) => blog.clone(),
        Some(Err(_)) => "-".to_string(),
        None => current_blog.clone(),
    };
    let handle = handle_for(
        state,
        "Article",
        &input,
        "article",
        Some(("blog", &scope)),
        Some(&id),
        &mut errors,
    );
    let (published, published_at) =
        publication(&input, &now, Some(&record), "article", &mut errors);
    let owned = check_owned_inputs(
        state,
        Some(&id),
        "ARTICLE",
        input.get("metafields"),
        &["article", "metafields"],
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(coded(
            json!({"article": null}),
            &errors,
            "ArticleUpdateUserErrorCode",
        ));
    }
    let old_handle = text(&record, "handle").to_string();
    let state = mutation.state_mut();
    let new_blog = match blog {
        Some(Ok(blog)) => Some(blog),
        Some(Err(title)) => {
            let mut blog_input = Map::new();
            blog_input.insert("title".into(), json!(title));
            let handle = handle_for(
                state,
                "Blog",
                &blog_input,
                "blog",
                None,
                None,
                &mut Vec::new(),
            )
            .unwrap_or_default();
            Some(insert_blog(state, &blog_input, &handle))
        }
        None => None,
    };
    let image = image_of(state, &input);
    if let Some(article) = state.get_mut(&id) {
        for field in ["body", "summary"] {
            if let Some(value) = input.get(field) {
                article.insert(field.into(), value.clone());
            }
        }
        if let Some(title) = input.get("title").and_then(Value::as_str) {
            article.insert("title".into(), json!(title.trim()));
        }
        if let Some(handle) = &handle {
            article.insert("handle".into(), json!(handle));
        }
        if let Some(author) = author {
            article.insert("author".into(), author);
        }
        if input.contains_key("tags") {
            article.insert("tags".into(), json!(tags_of(input.get("tags"))));
        }
        if input.contains_key("image") {
            article.insert("image".into(), image.unwrap_or(Value::Null));
        }
        if let Some(suffix) = template_suffix(&input) {
            article.insert("templateSuffix".into(), suffix);
        }
        article.insert("isPublished".into(), json!(published));
        article.insert("publishedAt".into(), published_at);
        if let Some(blog) = &new_blog {
            article.insert("blog".into(), json!(blog));
        }
    }
    if let Some(blog) = new_blog.as_ref().filter(|blog| **blog != current_blog) {
        if let Some(Value::Array(articles)) = state
            .get_mut(&current_blog)
            .and_then(|record| record.get_mut("articles"))
        {
            articles.retain(|article| article.as_str() != Some(id.as_str()));
        }
        if let Some(Value::Array(articles)) = state
            .get_mut(blog)
            .and_then(|record| record.get_mut("articles"))
        {
            articles.push(json!(id));
        }
        refresh_blog(state, blog);
    }
    refresh_blog(state, &current_blog);
    set_owned(state, &id, &owned);
    if let Some(handle) = handle.filter(|handle| *handle != old_handle)
        && input.get("redirectNewHandle").and_then(Value::as_bool) == Some(true)
    {
        let blog_handle = state
            .get(text(&record, "blog"))
            .map(|blog| text(blog, "handle").to_string())
            .unwrap_or_default();
        add_redirect(
            state,
            &format!("/blogs/{blog_handle}/{old_handle}"),
            &format!("/blogs/{blog_handle}/{handle}"),
        );
    }
    state.touch(&id);
    state.emit("articles/update", &id);
    Ok(payload(json!({"article": id}), &[]))
}

/// Removes an article, its comments and its metafields, and takes it out of its blog:
/// `articles/delete`.
fn remove_article(state: &mut ShopState, id: &str) {
    let Some(record) = state.get(id).cloned() else {
        return;
    };
    let blog = text(&record, "blog").to_string();
    for comment in gids(record.get("comments")) {
        state.remove(&comment);
    }
    state.delete(id, "articles/delete");
    remove_metafields(state, &record);
    if let Some(Value::Array(articles)) = state
        .get_mut(&blog)
        .and_then(|record| record.get_mut("articles"))
    {
        articles.retain(|article| article.as_str() != Some(id));
    }
    refresh_blog(state, &blog);
}

fn article_delete(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    if mutation.state().type_of(&id) != Some("Article") {
        let error = UserError::new(&["id"], "Article does not exist").code("NOT_FOUND");
        return Ok(payload(json!({"deletedArticleId": null}), &[error]));
    }
    remove_article(mutation.state_mut(), &id);
    Ok(payload(json!({"deletedArticleId": id}), &[]))
}

// ---- comments ----------------------------------------------------------------------------

/// `commentApprove`, `commentSpam`, `commentNotSpam`: the comment's new status.
fn moderate(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    status: &str,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    if mutation.state().type_of(&id) != Some("Comment") {
        let error = UserError::new(&["id"], "Comment does not exist").code("NOT_FOUND");
        return Ok(payload(json!({"comment": null}), &[error]));
    }
    let state = mutation.state_mut();
    let now = state.timestamp();
    if let Some(comment) = state.get_mut(&id) {
        let published = status == "PUBLISHED";
        if published && comment.get("publishedAt").is_none_or(Value::is_null) {
            comment.insert("publishedAt".into(), json!(now));
        } else if !published {
            comment.insert("publishedAt".into(), Value::Null);
        }
        comment.insert("status".into(), json!(status));
        comment.insert("isPublished".into(), json!(published));
    }
    state.touch(&id);
    state.emit("comments/update", &id);
    Ok(payload(json!({"comment": id}), &[]))
}

fn comment_delete(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    if mutation.state().type_of(&id) != Some("Comment") {
        let error = UserError::new(&["id"], "Comment does not exist").code("NOT_FOUND");
        return Ok(payload(json!({"deletedCommentId": null}), &[error]));
    }
    let state = mutation.state_mut();
    let article = state
        .get(&id)
        .map(|comment| text(comment, "article").to_string())
        .unwrap_or_default();
    state.delete(&id, "comments/delete");
    if let Some(Value::Array(comments)) = state
        .get_mut(&article)
        .and_then(|record| record.get_mut("comments"))
    {
        comments.retain(|comment| comment.as_str() != Some(id.as_str()));
    }
    Ok(payload(json!({"deletedCommentId": id}), &[]))
}

/// A payload whose error codes are those of the error type's code enum: a code the enum does not
/// have (one of the metafield checks') is its `INVALID`, else none.
fn coded(fields: Value, errors: &[UserError], code_enum: &str) -> Value {
    let errors: Vec<UserError> = errors
        .iter()
        .cloned()
        .map(|mut error| {
            error.code = super::fit_code(error.code.as_deref(), code_enum);
            error
        })
        .collect();
    payload(fields, &errors)
}
