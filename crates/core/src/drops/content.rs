//! `page`, `blog`, `article` and `comment`.

use std::any::Any;

use serde_json::{Value as Json, json};
use slt_liquid::{Object, Value};

use super::media::ImageDrop;
use super::metafield::MetafieldsDrop;
use super::product::iso;
use super::{Memo, PaginatedList, SiteRef, hash, strings, time_value};
use crate::store::{Article, Blog, Page};
use crate::util::handleize;

pub struct PageDrop {
    site: SiteRef,
    index: usize,
}

impl PageDrop {
    pub fn value(site: &SiteRef, index: usize) -> Value {
        Value::object(PageDrop {
            site: site.clone(),
            index,
        })
    }

    fn page(&self) -> &Page {
        &self.site.store.pages[self.index]
    }
}

pub fn page_url(site: &SiteRef, page: &Page) -> String {
    site.request.localized(&format!("/pages/{}", page.handle))
}

impl Object for PageDrop {
    fn type_name(&self) -> &str {
        "page"
    }

    fn get(&self, key: &str) -> Option<Value> {
        let page = self.page();
        Some(match key {
            "id" => Value::Int(page.id as i64),
            "object_type" => Value::str("page"),
            "title" => Value::from(&page.title),
            "handle" => Value::from(&page.handle),
            "content" => Value::from(&page.content),
            "author" => Value::from(&page.author),
            "url" => Value::from(page_url(&self.site, page)),
            "template_suffix" => page
                .template_suffix
                .as_ref()
                .map_or_else(Value::empty_string, Value::from),
            "published_at" => time_value(&self.site, page.published_at),
            "updated_at" => time_value(&self.site, page.updated_at),
            "metafields" => MetafieldsDrop::value(&self.site, &page.metafields),
            _ => return None,
        })
    }

    fn to_json(&self) -> Json {
        let page = self.page();
        json!({
            "id": page.id,
            "title": page.title,
            "handle": page.handle,
            "body_html": page.content,
            "author": page.author,
            "created_at": iso(&self.site, page.published_at),
            "updated_at": iso(&self.site, page.updated_at),
            "published_at": iso(&self.site, page.published_at),
            "template_suffix": page.template_suffix,
        })
    }

    fn identity(&self) -> Option<String> {
        Some(format!("page:{}", self.page().id))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct BlogDrop {
    site: SiteRef,
    index: usize,
    /// Tags from `/blogs/<handle>/tagged/<tag>`, for the blog the page is about.
    tags: Vec<String>,
    memo: Memo,
}

impl BlogDrop {
    pub fn value(site: &SiteRef, index: usize) -> Value {
        Self::tagged(site, index, Vec::new())
    }

    pub fn tagged(site: &SiteRef, index: usize, tags: Vec<String>) -> Value {
        Value::object(BlogDrop {
            site: site.clone(),
            index,
            tags,
            memo: Memo::default(),
        })
    }

    fn blog(&self) -> &Blog {
        &self.site.store.blogs[self.index]
    }

    fn article_indexes(&self) -> Vec<usize> {
        self.blog()
            .articles
            .iter()
            .enumerate()
            .filter(|(_, article)| {
                self.tags
                    .iter()
                    .all(|tag| article.tags.iter().any(|t| handleize(t) == handleize(tag)))
            })
            .map(|(index, _)| index)
            .collect()
    }
}

pub fn blog_url(site: &SiteRef, blog: &Blog) -> String {
    site.request.localized(&format!("/blogs/{}", blog.handle))
}

impl Object for BlogDrop {
    fn type_name(&self) -> &str {
        "blog"
    }

    fn get(&self, key: &str) -> Option<Value> {
        let site = &self.site;
        let blog = self.blog();
        Some(match key {
            "id" => Value::Int(blog.id as i64),
            "title" => Value::from(&blog.title),
            "handle" => Value::from(&blog.handle),
            "url" => Value::from(blog_url(site, blog)),
            "articles" => self.memo.get("articles", || {
                PaginatedList::value(
                    self.article_indexes()
                        .into_iter()
                        .map(|article| ArticleDrop::value(site, self.index, article))
                        .collect(),
                )
            }),
            "articles_count" => Value::from(self.article_indexes().len()),
            "all_tags" | "tags" => {
                let articles: Vec<&Article> = if key == "tags" {
                    self.article_indexes()
                        .into_iter()
                        .map(|index| &blog.articles[index])
                        .collect()
                } else {
                    blog.articles.iter().collect()
                };
                let mut tags: Vec<String> = Vec::new();
                for article in articles {
                    for tag in &article.tags {
                        if !tags.contains(tag) {
                            tags.push(tag.clone());
                        }
                    }
                }
                tags.sort_by_key(|tag| tag.to_lowercase());
                strings(&tags)
            }
            "comments_enabled?" => Value::Bool(blog.comments_enabled),
            "moderated?" => Value::Bool(blog.moderated),
            "template_suffix" => blog
                .template_suffix
                .as_ref()
                .map_or_else(Value::empty_string, Value::from),
            "metafields" => MetafieldsDrop::value(site, &blog.metafields),
            "next_article" | "previous_article" => Value::Nil,
            _ => return None,
        })
    }

    fn identity(&self) -> Option<String> {
        Some(format!("blog:{}", self.blog().id))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct ArticleDrop {
    site: SiteRef,
    blog: usize,
    index: usize,
    memo: Memo,
}

impl ArticleDrop {
    pub fn value(site: &SiteRef, blog: usize, index: usize) -> Value {
        Value::object(ArticleDrop {
            site: site.clone(),
            blog,
            index,
            memo: Memo::default(),
        })
    }

    fn article(&self) -> &Article {
        &self.site.store.blogs[self.blog].articles[self.index]
    }

    pub fn url(&self) -> String {
        article_url(
            &self.site,
            &self.site.store.blogs[self.blog],
            self.article(),
        )
    }
}

pub fn article_url(site: &SiteRef, blog: &Blog, article: &Article) -> String {
    site.request
        .localized(&format!("/blogs/{}/{}", blog.handle, article.handle))
}

impl Object for ArticleDrop {
    fn type_name(&self) -> &str {
        "article"
    }

    fn get(&self, key: &str) -> Option<Value> {
        let site = &self.site;
        let blog = &site.store.blogs[self.blog];
        let article = self.article();
        Some(match key {
            "id" => Value::Int(article.id as i64),
            "object_type" => Value::str("article"),
            "title" => Value::from(&article.title),
            // Shopify exposes the article handle prefixed with its blog's handle.
            "handle" => Value::from(format!("{}/{}", blog.handle, article.handle)),
            "author" => Value::from(&article.author),
            "content" => Value::from(&article.content),
            "excerpt" => Value::from(&article.excerpt),
            "excerpt_or_content" => Value::from(if article.excerpt.is_empty() {
                &article.content
            } else {
                &article.excerpt
            }),
            "url" => Value::from(self.url()),
            "image" => ImageDrop::optional(site, article.image.as_ref()),
            "tags" => strings(&article.tags),
            "comments" => self.memo.get("comments", || {
                PaginatedList::value(
                    article
                        .comments
                        .iter()
                        .map(|comment| {
                            hash([
                                ("id", Value::Int(comment.id as i64)),
                                ("author", Value::from(&comment.author)),
                                ("email", Value::from(&comment.email)),
                                ("content", Value::from(&comment.content)),
                                ("status", Value::str("published")),
                                ("created_at", time_value(site, comment.created_at)),
                                ("updated_at", time_value(site, comment.created_at)),
                                ("url", Value::from(format!("{}#{}", self.url(), comment.id))),
                            ])
                        })
                        .collect(),
                )
            }),
            "comments_count" => Value::from(article.comments.len()),
            "comments_enabled?" => Value::Bool(blog.comments_enabled),
            "moderated?" => Value::Bool(blog.moderated),
            "comment_post_url" => Value::from(format!("{}/comments", self.url())),
            "template_suffix" => article
                .template_suffix
                .as_ref()
                .map_or_else(Value::empty_string, Value::from),
            "created_at" => time_value(site, article.created_at),
            "published_at" => time_value(site, article.published_at),
            "updated_at" => time_value(site, article.updated_at),
            "metafields" => MetafieldsDrop::value(site, &article.metafields),
            "user" => {
                let (first_name, last_name) = article
                    .author
                    .split_once(' ')
                    .unwrap_or((&article.author, ""));
                hash([
                    ("name", Value::from(&article.author)),
                    ("first_name", Value::from(first_name)),
                    ("last_name", Value::from(last_name)),
                    ("bio", Value::Nil),
                    ("email", Value::Nil),
                    ("homepage", Value::Nil),
                    ("image", Value::Nil),
                    ("account_owner", Value::Bool(false)),
                ])
            }
            _ => return None,
        })
    }

    fn identity(&self) -> Option<String> {
        Some(format!("article:{}", self.article().id))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
