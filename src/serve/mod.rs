use axum::extract::{Json, Path, Query};
use axum::response::IntoResponse;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use itertools::Itertools;
use log::debug;
use log::error;
use log::info;
use reqwest::header::CONTENT_TYPE;
use serde_json::{json, Value};
use serde_json_path::JsonPath;
use std::error::Error;
use std::io::{Read, Write};
use uuid::Uuid;
use zip::ZipArchive;

use crate::conf;
use crate::convert::{get_format_content_type, FB2C_CONVERTER};
use crate::serve::request::{ArchiveRequest, Author, DownloadFormat, Search};

pub(crate) mod request;

lazy_static::lazy_static! {
    static ref ES_CLIENT: EsClient = EsClient::new().unwrap();
}

pub async fn start() -> Result<(), Box<dyn Error>> {
    let addr;
    {
        let settings = conf::SETTINGS.read()?;
        addr = settings.listen_address.clone();
    }

    let app = build_router();

    info!("Serving the API on {}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

fn build_router() -> axum::Router {
    Router::new()
        .route("/api/author/search", post(authors_handler))
        .route("/api/author/books", post(authors_books_handler))
        .route("/api/book/langs", get(langs_handler))
        .route("/api/book/search", post(title_search_handler))
        .route("/api/book/series", post(series_search_handler))
        .route("/api/book/{id}", get(info_handler))
        .route("/api/book/{id}/download", get(download_handler))
        .route("/api/book/archive", post(archive_handler))
        .route("/api/book/archive", get(archive_get_handler))
}

struct EsClient {
    client: reqwest::Client,
    url: String,
    login: String,
    password: String,
}

impl EsClient {
    fn new() -> Result<Self, String> {
        let settings = conf::SETTINGS.read();
        let s = match settings {
            Ok(s) => s,
            Err(_) => return Err("Failed to read settings".to_string()),
        };
        let url = s.elastic_url.clone();
        let login = s.elastic_login.clone();
        let password = s.elastic_password.clone();

        Ok(EsClient {
            client: reqwest::Client::new(),
            url,
            login,
            password,
        })
    }

    async fn search(&self, index: &str, body: Value) -> Result<Value, String> {
        let url = format!("{}/{}/_search", self.url, index);
        debug!("ES search: url={}, body={}", url, body);
        let response = self
            .client
            .post(&url)
            .header(CONTENT_TYPE, "application/json")
            .basic_auth(&self.login, Some(&self.password))
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let status = response.status();
        if status.is_success() {
            let result = response.json().await.map_err(|e| e.to_string())?;
            debug!("ES search success: status={}, response={}", status, result);
            Ok(result)
        } else {
            Err(format!("Search failed: {}", status))
        }
    }

    async fn get(&self, index: &str, id: &str) -> Result<Value, String> {
        let url = format!("{}/{}/_doc/{}", self.url, index, id);
        debug!("ES get: url={}", url);
        let response = self
            .client
            .get(&url)
            .basic_auth(&self.login, Some(&self.password))
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if response.status().is_success() {
            let body: Value = response.json().await.map_err(|e| e.to_string())?;
            if let Some(source) = body.get("_source") {
                let result = source.clone();
                debug!("ES get success: id={}, response={}", id, result);
                Ok(result)
            } else {
                Err("Document not found".to_string())
            }
        } else {
            Err("Document not found".to_string())
        }
    }
}

async fn es_search(query: Value, path: &str) -> Result<String, String> {
    let index = {
        let s = conf::SETTINGS.read();
        match s {
            Ok(s) => s.elastic_index.clone(),
            Err(_) => return Err("Failed to read settings".to_string()),
        }
    };

    debug!("ES query: index={}, path={}, query={}", index, path, query);

    let result = ES_CLIENT
        .search(&index, query)
        .await
        .map_err(|e| e.to_string())?;

    debug!("ES response: {}", result);

    let json_path = JsonPath::parse(path).map_err(|e| e.to_string())?;
    let found = json_path.query(&result).all();

    if found.is_empty() {
        Err("No matched data found".to_string())
    } else {
        // If we have exactly one node and it's an array, return its contents directly
        // to avoid double-wrapping arrays
        let data_to_serialize: Vec<Value> = if found.len() == 1 {
            found[0].as_array().map(|arr| arr.to_vec())
        } else {
            None
        }
        .unwrap_or_else(|| found.into_iter().cloned().collect());

        // Transform hits to array of {id, book} objects if path is $.hits.hits
        let transformed = if path == "$.hits.hits" {
            data_to_serialize
                .into_iter()
                .map(|hit| {
                    let id = hit.get("_id").cloned().unwrap_or(Value::Null);
                    let book = hit.get("_source").cloned().unwrap_or(Value::Null);
                    json!({"id": id, "book": book})
                })
                .collect()
        } else {
            data_to_serialize
        };

        Ok(serde_json::to_string(&transformed).map_err(|e| e.to_string())?)
    }
}

fn make_term<F>(query: &str, closure: F)
where
    F: FnMut(String),
{
    query
        .split_whitespace()
        .map(|v| format!("*{}*", v.to_lowercase()))
        .for_each(closure);
}

fn compose_es_request(search: &Search, s_type: SearchType) -> Value {
    let del = if search.deleted { 1 } else { 0 };

    let mut req = json!({
        "size": search.limit,
        "sort": [],
        "query": {
            "bool": {
                "filter": [{
                    "terms": {
                        "del": [0, del]
                    }
                }]
            }
        }
    });

    let mut filters = match s_type {
        SearchType::TitlesSearch => {
            let mut vec = Vec::new();
            make_term(&search.author, |term| {
                vec.push(json!({"wildcard": {"authors": term}}))
            });
            make_term(&search.title, |term| {
                vec.push(json!({"wildcard": {"title": term}}))
            });
            vec
        }
        SearchType::SeriesSearch => {
            let mut vec = Vec::new();
            make_term(&search.author, |term| {
                vec.push(json!({"wildcard": {"authors": term}}))
            });
            make_term(&search.series, |term| {
                vec.push(json!({"wildcard": {"series": term}}))
            });
            vec
        }
        SearchType::AuthorsBooks => {
            let mut vec = Vec::new();
            vec.push(json!({
                "match_phrase_prefix": {
                    "authors": search.author
                }
            }));
            vec
        }
    };

    if !search.langs.is_empty() {
        filters.push(json!({
            "terms": {
                "lang": search.langs
            }
        }));
    }

    req["query"].as_object_mut().unwrap()["bool"]
        .as_object_mut()
        .unwrap()["filter"]
        .as_array_mut()
        .unwrap()
        .append(&mut filters);

    let mut sort = match s_type {
        SearchType::SeriesSearch | SearchType::AuthorsBooks => {
            vec![
                json!("series.keyword"),
                json!("ser_no.keyword"),
                json!("title.keyword"),
            ]
        }
        SearchType::TitlesSearch => vec![json!("title.keyword")],
    };

    req["sort"].as_array_mut().unwrap().append(&mut sort);

    req
}

async fn langs_handler() -> impl IntoResponse {
    let query = json!({
        "size": 0,
        "query": {
            "match": {
                "del": "0"
            }
        },
        "aggs": {
            "lang": {
                "terms": {
                    "field": "lang.keyword",
                    "include": ".*",
                    "size": 100
                }
            }
        }
    });

    match es_search(query, "$.aggregations.lang.buckets..key").await {
        Ok(search_result) => match serde_json::from_str::<Value>(&search_result) {
            Ok(body) => (axum::http::StatusCode::OK, Json(body)),
            Err(_) => (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Failed to parse response"})),
            ),
        },
        Err(e) => (
            axum::http::StatusCode::NOT_FOUND,
            Json(json!([{"error": e.to_string()}])),
        ),
    }
}

async fn authors_handler(Json(author_req): Json<Author>) -> impl IntoResponse {
    let search_query = format!(
        ".*{}.*",
        author_req
            .author
            .split_whitespace()
            .map(|v| {
                let mut chars = v.chars();
                match chars.next() {
                    None => String::new(),
                    Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
                }
            })
            .join(".*")
    );

    let query = json!({
        "size": 0,
        "aggs": {
            "author": {
                "terms": {
                    "field": "authors.keyword",
                    "include": search_query,
                    "size": author_req.limit
                }
            }
        }
    });

    match es_search(query, "$.aggregations.author.buckets..key").await {
        Ok(search_result) => match serde_json::from_str::<Value>(&search_result) {
            Ok(body) => (axum::http::StatusCode::OK, Json(body)),
            Err(_) => (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Failed to parse response"})),
            ),
        },
        Err(e) => (
            axum::http::StatusCode::NOT_FOUND,
            Json(json!([{"error": e.to_string()}])),
        ),
    }
}

async fn authors_books_handler(Json(search): Json<Search>) -> impl IntoResponse {
    let req = compose_es_request(&search, SearchType::AuthorsBooks);

    match es_search(req, "$.hits.hits").await {
        Ok(search_result) => match serde_json::from_str(&search_result) {
            Ok(body) => (axum::http::StatusCode::OK, Json(body)),
            Err(_) => (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Failed to parse response"})),
            ),
        },
        Err(e) => (
            axum::http::StatusCode::NOT_FOUND,
            Json(json!({"error": e.to_string()})),
        ),
    }
}

async fn title_search_handler(Json(search): Json<Search>) -> impl IntoResponse {
    let req = compose_es_request(&search, SearchType::TitlesSearch);

    match es_search(req, "$.hits.hits").await {
        Ok(search_result) => match serde_json::from_str(&search_result) {
            Ok(body) => (axum::http::StatusCode::OK, Json(body)),
            Err(_) => (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Failed to parse response"})),
            ),
        },
        Err(e) => (
            axum::http::StatusCode::NOT_FOUND,
            Json(json!({"error": e.to_string()})),
        ),
    }
}

async fn series_search_handler(Json(search): Json<Search>) -> impl IntoResponse {
    let req = compose_es_request(&search, SearchType::SeriesSearch);

    match es_search(req, "$.hits.hits").await {
        Ok(search_result) => match serde_json::from_str(&search_result) {
            Ok(body) => (axum::http::StatusCode::OK, Json(body)),
            Err(_) => (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Failed to parse response"})),
            ),
        },
        Err(e) => (
            axum::http::StatusCode::NOT_FOUND,
            Json(json!({"error": e.to_string()})),
        ),
    }
}

async fn info_handler(Path(book_id): Path<String>) -> impl IntoResponse {
    match ES_CLIENT.get("flibooks", &book_id).await {
        Ok(nfo) => {
            let body = Json(nfo);
            (axum::http::StatusCode::OK, body).into_response()
        }
        Err(_) => {
            let body = Json(json!({"error": "Book not found"}));
            (axum::http::StatusCode::NOT_FOUND, body).into_response()
        }
    }
}

async fn download_handler(
    Path(book_id): Path<String>,
    Query(params): Query<DownloadFormat>,
) -> impl IntoResponse {
    let nfo = match ES_CLIENT.get("flibooks", &book_id).await {
        Ok(n) => n,
        Err(_) => {
            let body = Json(json!({"error": "Book not found"}));
            return (axum::http::StatusCode::NOT_FOUND, body).into_response();
        }
    };

    let container = nfo["container"].as_str().unwrap();
    let file_name = format!(
        "{}.{}",
        nfo["file"].as_str().unwrap(),
        nfo["ext"].as_str().unwrap()
    );
    let out_name = get_out_file_name(&nfo);
    let format = params.format;

    // Check if conversion is requested
    if !format.is_empty() {
        return download_converted_book(container, &file_name, &out_name, &format).await;
    }

    // Download original file
    match get_book_file(container, &file_name).await {
        Ok(book_content) => {
            let mut response = Response::new(axum::body::Body::from(book_content));
            response.headers_mut().insert(
                axum::http::header::CONTENT_DISPOSITION,
                axum::http::header::HeaderValue::from_str(&format!(
                    "attachment; filename=\"{}\"",
                    out_name
                ))
                .unwrap(),
            );
            response.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                axum::http::header::HeaderValue::from_static("application/fb2"),
            );
            response
        }
        Err(e) => {
            let body = Json(json!({"error": e.to_string()}));
            (axum::http::StatusCode::NOT_FOUND, body).into_response()
        }
    }
}

async fn download_converted_book(
    container: &str,
    file_name: &str,
    out_name: &str,
    format: &str,
) -> Response<axum::body::Body> {
    // Check if format is supported
    if get_format_content_type(format).is_none() {
        let body = Json(
            json!({"error": format!("Unsupported format: {}. Supported formats: epub, azw3, mobi", format)}),
        );
        return (axum::http::StatusCode::BAD_REQUEST, body).into_response();
    }

    let temp_dir = match tempfile::tempdir() {
        Ok(d) => d,
        Err(e) => {
            let body = Json(json!({"error": e.to_string()}));
            return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, body).into_response();
        }
    };

    let src_path = temp_dir.path().join("file.fb2");

    // Extract FB2 file to temp location
    let book_content = match get_book_file(container, file_name).await {
        Ok(content) => content,
        Err(e) => {
            let body = Json(json!({"error": e.to_string()}));
            return (axum::http::StatusCode::NOT_FOUND, body).into_response();
        }
    };

    if let Err(e) = std::fs::write(&src_path, &book_content) {
        let body = Json(json!({"error": e.to_string()}));
        return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, body).into_response();
    }

    // Convert using fb2c
    match FB2C_CONVERTER.convert(
        src_path.to_str().unwrap(),
        temp_dir.path().to_str().unwrap(),
        format,
    ) {
        Ok(_) => (),
        Err(e) => {
            let body = Json(json!({"error": format!("Conversion failed: {}", e)}));
            return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, body).into_response();
        }
    };

    // Read converted file
    let converted_path = temp_dir.path().join(format!("file.{}", format));
    let converted_content = match std::fs::read(&converted_path) {
        Ok(content) => content,
        Err(e) => {
            let body = Json(json!({"error": format!("Failed to read converted file: {}", e)}));
            return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, body).into_response();
        }
    };

    let conv_name = format!("{}.{}", out_name, format);
    let content_type = get_format_content_type(format).unwrap();

    let mut response = Response::new(axum::body::Body::from(converted_content));
    response.headers_mut().insert(
        axum::http::header::CONTENT_DISPOSITION,
        axum::http::header::HeaderValue::from_str(&format!(
            "attachment; filename=\"{}\"",
            conv_name
        ))
        .unwrap(),
    );
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::header::HeaderValue::from_static(content_type),
    );
    response
}

async fn archive_handler(Json(archive_request): Json<ArchiveRequest>) -> impl IntoResponse {
    let index = {
        let s = conf::SETTINGS.read();
        match s {
            Ok(s) => s.elastic_index.clone(),
            Err(_) => {
                let body = Json(json!({"error": "Failed to read settings"}));
                return (axum::http::StatusCode::BAD_REQUEST, body).into_response();
            }
        }
    };

    let format = archive_request.format;
    let temp_dir = match tempfile::tempdir() {
        Ok(d) => d,
        Err(e) => {
            let body = Json(json!({"error": e.to_string()}));
            return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, body).into_response();
        }
    };
    let dir = temp_dir.path().join(format!("flibooks-{}", Uuid::new_v4()));
    if let Err(e) = std::fs::create_dir_all(&dir) {
        let body = Json(json!({"error": e.to_string()}));
        return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, body).into_response();
    }

    for id in &archive_request.id {
        match ES_CLIENT.get(&index, id).await {
            Ok(nfo) => {
                let container = nfo["container"].as_str().unwrap();
                let file_name = format!(
                    "{}.{}",
                    nfo["file"].as_str().unwrap(),
                    nfo["ext"].as_str().unwrap()
                );
                let out_name = get_out_file_name(&nfo);
                match get_book_file(container, &file_name).await {
                    Ok(book_content) => {
                        if format.is_empty() {
                            // Download original file without conversion
                            match std::fs::File::create(dir.join(&out_name)) {
                                Ok(mut file) => {
                                    if let Err(e) = file.write_all(&book_content) {
                                        error!("Cannot write book {}: {}", id, e);
                                    }
                                }
                                Err(e) => error!("Cannot create file for {}: {}", id, e),
                            }
                        } else {
                            // Convert book to specified format
                            process_converted_book(&dir, &out_name, &format, &book_content, id)
                                .await;
                        }
                    }
                    Err(e) => error!("Cannot find the book {}: {}", id, e),
                }
            }
            Err(e) => error!("Cannot find the book {}: {}", id, e),
        }
    }

    // Create zip archive
    let zip_name = format!("{}.zip", dir.to_string_lossy());
    let result = create_zip(&dir, &zip_name);
    match result {
        Ok(_) => match std::fs::read(&zip_name) {
            Ok(zip_bytes) => {
                let mut response = Response::new(axum::body::Body::from(zip_bytes));
                response.headers_mut().insert(
                    axum::http::header::CONTENT_DISPOSITION,
                    axum::http::header::HeaderValue::from_str(&format!(
                        "attachment; filename=\"{}\"",
                        zip_name.split('/').next_back().unwrap_or("archive.zip")
                    ))
                    .unwrap(),
                );
                response.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    axum::http::header::HeaderValue::from_static("application/zip"),
                );
                response
            }
            Err(e) => {
                let body = Json(json!({"error": e.to_string()}));
                (axum::http::StatusCode::INTERNAL_SERVER_ERROR, body).into_response()
            }
        },
        Err(e) => {
            let body = Json(json!({"error": e.to_string()}));
            (axum::http::StatusCode::INTERNAL_SERVER_ERROR, body).into_response()
        }
    }
}

// Use custom extractor for archive GET endpoint to handle repeated ids query params
// Supports both ?ids=val1&ids=val2 and ?ids[]=val1&ids[]=val2 formats
async fn archive_get_handler(request: axum::extract::Request) -> axum::response::Response {
    // Extract query string from request
    let query_string = match request.uri().query() {
        Some(q) => q,
        None => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                axum::Json(json!({"error": "Missing query string"})),
            )
                .into_response();
        }
    };

    let mut id = Vec::new();
    let mut format = String::new();

    for (k, v) in url::form_urlencoded::parse(query_string.as_bytes()) {
        if k == "id" || k == "id[]" {
            id.push(v.into_owned());
        } else if k == "format" {
            format = v.into_owned();
        }
    }

    let archive_request = ArchiveRequest { id, format };
    archive_handler(axum::extract::Json(archive_request))
        .await
        .into_response()
}

async fn process_converted_book(
    dir: &std::path::Path,
    out_name: &str,
    format: &str,
    book_content: &[u8],
    id: &str,
) {
    let temp_dir = match tempfile::tempdir() {
        Ok(d) => d,
        Err(e) => {
            error!("Cannot create temp dir for book {}: {}", id, e);
            return;
        }
    };

    let src_path = temp_dir.path().join("file.fb2");

    // Write FB2 file
    if let Err(e) = std::fs::write(&src_path, book_content) {
        error!("Cannot write temp file for book {}: {}", id, e);
        return;
    }

    // Convert using fb2c
    let format_ext = format;
    let src_str = src_path.to_str().unwrap();
    let dst_str = temp_dir.path().to_str().unwrap();
    debug!(
        "Converting book {}: src={}, dst_dir={}, format={}",
        id, src_str, dst_str, format
    );
    if let Err(e) = FB2C_CONVERTER.convert(src_str, dst_str, format) {
        error!("Cannot convert book {}: {}", id, e);
        return;
    }

    // Read converted file
    let converted_path = temp_dir.path().join(format!("file.{}", format_ext));
    if let Ok(content) = std::fs::read(&converted_path) {
        let conv_name = format!("{}.{}", out_name, format_ext);
        if let Ok(mut file) = std::fs::File::create(dir.join(&conv_name)) {
            let _ = file.write_all(&content);
        } else {
            error!(
                "Cannot create converted file for book {}: {}",
                id, conv_name
            );
        }
    } else {
        error!("Cannot read converted file for book {}: {}", id, format_ext);
    }
}

async fn get_book_file(container: &str, file: &str) -> Result<Vec<u8>, String> {
    let container_file = std::fs::File::open(container).map_err(|e| e.to_string())?;
    let mut archive = ZipArchive::new(container_file).map_err(|e| e.to_string())?;
    let mut book_content = archive.by_name(file).map_err(|e| e.to_string())?;

    let mut buffer = Vec::new();
    std::io::Read::read_to_end(&mut book_content, &mut buffer).map_err(|e| e.to_string())?;
    Ok(buffer)
}

fn create_zip(dir: &std::path::Path, zip_path: &str) -> Result<(), String> {
    let file = std::fs::File::create(zip_path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(file);
    let options: zip::write::FileOptions<()> = zip::write::FileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o755);

    let mut buffer = Vec::new();
    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        let name = path
            .strip_prefix(dir)
            .map_err(|_| "Failed to strip prefix")?
            .to_str()
            .ok_or("Invalid path")?;

        if path.is_file() {
            zip.start_file(name, options).map_err(|e| e.to_string())?;
            let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
            f.read_to_end(&mut buffer).map_err(|e| e.to_string())?;
            zip.write_all(&buffer).map_err(|e| e.to_string())?;
            buffer.clear();
        } else if !name.is_empty() {
            zip.add_directory(name, options)
                .map_err(|e| e.to_string())?;
        }
    }
    zip.finish().map_err(|e| e.to_string())?;
    Ok(())
}

fn truncate(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        None => s.to_string(),
        Some((idx, _)) => format!("{}…", &s[..idx]),
    }
}

fn get_out_file_name(nfo: &Value) -> String {
    let title = nfo["title"].as_str().unwrap();
    let trim_chars: &[char] = &[',', ' '];

    let auth_vec: Vec<&str> = nfo["authors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap().trim_matches(trim_chars))
        .collect();

    let mut authors = auth_vec.join(", ");
    authors = truncate(&authors, 100);

    let series = nfo["series"].as_str().unwrap_or("");
    let ser = nfo["ser_no"].as_str().unwrap_or("");
    if !series.is_empty() && !ser.is_empty() {
        return format!("{} - [{}] {}.fb2", authors, ser, title);
    }

    format!("{} - {}.fb2", authors, title)
}

enum SearchType {
    AuthorsBooks,
    TitlesSearch,
    SeriesSearch,
}
