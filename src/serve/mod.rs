use bodyparser;
use elastic::prelude::*;
use iron::headers::{ContentDisposition, ContentType, DispositionType, DispositionParam, Charset};
use iron::mime::{Mime, TopLevel, SubLevel};
use iron::modifiers::Header;
use iron::prelude::*;
use iron::status;
use jsonpath::Selector;
use persistent;
use router::Router;
use serde_json;
use serde_json::Value;
use std::borrow::Cow;
use std::error::Error;
use std::fs::File;
use std::io;
use std::io::Read;
use std::iter::Iterator;
use zip;

use conf;

pub mod request;

const MAX_BODY_LENGTH: usize = 1024 * 1024 * 10;

enum SearchType {
    AuthorsBooks,
    TitlesSearch,
    SeriesSearch,
}

lazy_static! {
    pub static ref ES: SyncClient = es_connect().unwrap();
}

pub fn start() -> Result<(), Box<Error>> {
    let settings = conf::SETTINGS.read()?;

    let listen = &settings.listen_address;

    let router = router!{
        authors:        post "/api/author/search"       => authors_handler,
        author_books:   post "/api/author/books"        => authors_books_handler,
        langs:          get  "/api/book/langs"          => langs_handler,
        title_search:   post "/api/book/search"         => title_search_handler,
        series_search:  post "/api/book/series"         => series_search_handler,
        book_info:      get  "/api/book/:id"            => info_handler,
        book_download:  get  "/api/book/:id/download"   => download_handler,
    };

    let mut chain = Chain::new(router);
    chain.link_before(persistent::Read::<bodyparser::MaxBodyLength>::one(
        MAX_BODY_LENGTH,
    ));

    info!("Serving the API on {}", listen);
    Iron::new(chain).http(listen)?;

    Ok(())
}

pub fn es_connect() -> Result<SyncClient, Box<Error>> {
    match conf::SETTINGS.read() {
        Ok(settings) => {
            let base_url = settings.elastic_url.as_str();

            info!("Using the elasticsearch at '{}'", base_url);

            Ok(SyncClientBuilder::new().base_url(base_url).build()?)
        }
        _ => Err(From::from("Failed to read settings")),
    }
}

fn es_search(query: Value, filter: &str) -> Result<String, Box<Error>> {
    let mut result = String::new();

    let settings = conf::SETTINGS.read()?;

    let req = {
        let body = query;
        SearchRequest::for_index_ty(
            Cow::Borrowed(settings.elastic_index.as_str()).into_owned(),
            "book", body)
    };

    let mut raw = String::new();
    ES.request(req)
        .send()?
        .into_raw()
        .read_to_string(&mut raw)?;
    let json: Value = serde_json::from_str(raw.as_str())?;

    let selector = Selector::new(filter)?;
    let mut found = selector.find(&json);
    match found.next() {
        Some(filtered) => {
            result.push_str(&serde_json::to_string(&(*filtered))?.to_string());
        }
        None => return Err(From::from("No matched data found")),
    }

    Ok(result)
}

fn make_term<F>(query: &String, closure: F)
    where F: FnMut(String) {
        query.split_whitespace().map(|v| {
            let mut out = String::from("*");
            out.push_str(&v.to_lowercase());
            out.push_str("*");
            out
        }).for_each(closure);
}

fn compose_es_request(search: &request::Search, s_type: SearchType) -> serde_json::Value {
    let del = if search.deleted { 1 } else { 0 };

    let mut req = json!({
        "size": search.limit,
        "sort": [
        ],
        "query": {
            "bool": {
                "filter": [{
                    "terms": {
                        "del": [ 0, del ]
                    }
                }]
            }
    }});

    // setup filters
    let mut filters = match s_type {
        SearchType::TitlesSearch => {
            let mut vec = Vec::new();
            make_term( &search.author, |term| vec.push(json!({"wildcard": { "authors": term }})) );
            make_term( &search.title,  |term| vec.push(json!({"wildcard": { "title":   term }})) );
            vec
        }
        SearchType::SeriesSearch => {
            let mut vec = Vec::new();
            make_term( &search.author, |term| vec.push(json!({"wildcard": { "authors": term }})) );
            make_term( &search.series, |term| vec.push(json!({"wildcard": { "series":  term }})) );
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
            let mut vec = Vec::new();
            vec.push(json!("series.keyword"));
            vec.push(json!("ser_no"));
            vec.push(json!("title.keyword"));
            vec
        }
        SearchType::TitlesSearch => {
            let mut vec = Vec::new();
            vec.push(json!("title.keyword"));
            vec
        }
    };

    req["sort"].as_array_mut().unwrap().append(&mut sort);

    req
}

fn langs_handler(_req: &mut Request) -> IronResult<Response> {
    let mut _error_response = Response::with((status::BadRequest, "Server Error"));
    debug!("Get langs request");

    // TODO: add ability to include langs for deleted books too
    match es_search(
        json!({
            "size": 0,
            "query": {
                "match" : {
                    "del":"0"
                }
            },
            "aggs": {
                "lang": {
                    "terms": {
                        "field": "lang.keyword",
                        "include":  ".*",
                        "size": 100
                    }
                }
        }}),
        "$.aggregations.lang.buckets",
    ) {
        Ok(search_result) => {
            return Ok(Response::with((
                status::Ok,
                Header(ContentType::json()),
                search_result,
            )));
        }
        Err(e) => _error_response = Response::with((status::NotFound, e.to_string())),
    }

    error!("Responding with error: {}", _error_response);
    Ok(_error_response)
}

fn authors_handler(req: &mut Request) -> IronResult<Response> {
    let body = req.get::<bodyparser::Struct<request::Author>>();
    let mut _error_response = Response::with((status::BadRequest, "Server Error"));

    match body {
        Ok(Some(author_req)) => {
            debug!("Search author request:\n{:?}", author_req);

            use itertools::join;
            use inflections::case::to_title_case;

            let search_query = format!(".*{}.*", join(author_req.author.split_whitespace().map(|v| to_title_case(v)), ".*"));

            match es_search(
                json!({
                    "size": 0,
                    "aggs": {
                        "author": {
                        "terms": {
                            "field": "authors.keyword",
                            "include": search_query,
                            "size": author_req.limit
                            }
                        }
                }}),
                "$.aggregations.author.buckets",
            ) {
                Ok(search_result) => {
                    return Ok(Response::with((
                        status::Ok,
                        Header(ContentType::json()),
                        search_result,
                    )));
                }
                Err(e) => _error_response = Response::with((status::NotFound, e.to_string())),
            }
        }
        _ => _error_response = Response::with((status::BadRequest, "Failed to parse request")),
    }

    error!("Responding with error: {}", _error_response);
    Ok(_error_response)
}

fn authors_books_handler(req: &mut Request) -> IronResult<Response> {
    let mut _error_response = Response::with((status::BadRequest, "Server Error"));
    let body = req.get::<bodyparser::Struct<request::Search>>();

    match body {
        Ok(Some(search)) => {
            debug!("Author's books request:\n{:?}", &search);
            let req = compose_es_request(&search, SearchType::AuthorsBooks);

            match es_search(req, "$.hits.hits") {
                Ok(search_result) => {
                    return Ok(Response::with((
                        status::Ok,
                        Header(ContentType::json()),
                        search_result,
                    )));
                }
                Err(e) => _error_response = Response::with((status::NotFound, e.to_string())),
            }
        }
        _ => {
            _error_response = Response::with((status::BadRequest, "Failed to parse search request"))
        }
    }

    error!("Responding with error: {}", _error_response);
    Ok(_error_response)
}

fn title_search_handler(req: &mut Request) -> IronResult<Response> {
    let mut _error_response = Response::with((status::BadRequest, "Server Error"));
    let body = req.get::<bodyparser::Struct<request::Search>>();
    match body {
        Ok(Some(search)) => {
            debug!("Books title search:\n{:?}", &search);
            let req = compose_es_request(&search, SearchType::TitlesSearch);
            match es_search(req, "$.hits.hits") {
                Ok(search_result) => {
                    return Ok(Response::with((
                        status::Ok,
                        Header(ContentType::json()),
                        search_result,
                    )));
                }
                Err(e) => _error_response = Response::with((status::NotFound, e.to_string())),
            }
        }
        _ => {
            _error_response = Response::with((status::BadRequest, "Failed to parse search request"))
        }
    }
    error!("Responding with error: {}", _error_response);
    Ok(_error_response)
}

fn series_search_handler(req: &mut Request) -> IronResult<Response> {
    let mut _error_response = Response::with((status::BadRequest, "Server Error"));
    let body = req.get::<bodyparser::Struct<request::Search>>();
    match body {
        Ok(Some(search)) => {
            debug!("Books series search:\n{:?}", &search);
            let req = compose_es_request(&search, SearchType::SeriesSearch);
            match es_search(req, "$.hits.hits") {
                Ok(search_result) => {
                    return Ok(Response::with((
                        status::Ok,
                        Header(ContentType::json()),
                        search_result,
                    )));
                }
                Err(e) => _error_response = Response::with((status::NotFound, e.to_string())),
            }
        }
        _ => {
            _error_response = Response::with((status::BadRequest, "Failed to parse search request"))
        }
    }
    error!("Responding with error: {}", _error_response);
    Ok(_error_response)
}


fn info_handler(req: &mut Request) -> IronResult<Response> {
    let mut _error_response = Response::with((status::BadRequest, "Server Error"));
    let id = req.extensions.get::<Router>().unwrap().find("id").unwrap();

    info!("Requesting book with id {}", id);

    match book_info(id) {
        Ok(nfo) => {
            return Ok(Response::with((
                status::Ok,
                Header(ContentType::json()),
                nfo.to_string(),
            )));
        }
        Err(e) => _error_response = Response::with((status::NotFound, e.to_string())),
    }

    error!("Responding with error: {}", _error_response);
    Ok(_error_response)
}

fn download_handler(req: &mut Request) -> IronResult<Response> {
    let mut _error_response = Response::with((status::BadRequest, "Server Error"));
    let id = req.extensions.get::<Router>().unwrap().find("id").unwrap();

    info!("Downloading book with id {}", id);

    match book_info(id) {
        Ok(nfo) => {
            let container = nfo["container"].as_str().unwrap();
            let file_name = format!("{}.{}", nfo["file"].as_str().unwrap(), nfo["ext"].as_str().unwrap());

            let out_name = get_out_file_name(&nfo);

            use tempfile::tempdir;
            let dir = tempdir().unwrap();
            {
                let mut file = File::create(dir.path().join(file_name.as_str())).unwrap();
                unpack_book(container, file_name.as_str(), &mut file).unwrap();
            }
            let file = dir.path().join(file_name.as_str());
            let mut resp = Response::with((status::Ok, file));

            resp.headers.set(ContentType(Mime(TopLevel::Application, SubLevel::OctetStream, vec![])));
            resp.headers.set(ContentDisposition {
                disposition: DispositionType::Attachment,
                parameters: vec![DispositionParam::Filename(
                    Charset::Ext("utf8".to_string()),
                    None,
                    out_name.as_bytes().to_vec()
                )]
            });
            return Ok(resp);
        }
        Err(e) => _error_response = Response::with((status::NotFound, e.to_string())),
    }

    error!("Responding with error: {}", _error_response);
    Ok(_error_response)
}

fn book_info(book_id: &str) -> Result<serde_json::Value, Box<Error>> {
    let settings = conf::SETTINGS.read()?;

    ES.document_get::<Value>(
        index(Cow::Borrowed(settings.elastic_index.as_str()).into_owned()),
        id(Cow::Borrowed(book_id).into_owned())
    ).ty("book").send()?.into_document().ok_or(From::from("No matched data found"))
}

fn truncate(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        None => String::from(s),
        Some((idx, _)) => format!("{}…", &s[..idx])
    }
}

fn get_out_file_name(nfo: &Value) -> String {
    let title = nfo["title"].as_str().unwrap();

    let trim_chars: &[char] = &[',', ' '];
    let auth_vec: Vec<&str> = nfo["authors"]
            .as_array().unwrap()
            .iter()
                .map(|a| a.as_str().unwrap().trim_matches(trim_chars))
                .collect();

    let mut authors = auth_vec.join(", ");
    authors = truncate(&authors, 100);

    if nfo["ser_no"].is_i64() {
        let ser = nfo["ser_no"].as_i64().unwrap();
        if ser > 0 {
            return format!("{} - [{}] {}", authors, ser, title);
        }
    }

    format!("{} - {}.fb2", authors, title)
}

fn unpack_book(container: &str, file: &str, out_file: &mut File) -> Result<(), Box<Error>> {
    let container_file = File::open(container)?;
    let mut archive = zip::ZipArchive::new(container_file)?;
    let mut book_content = archive.by_name(file)?;

    io::copy(&mut book_content, out_file)?;
    Ok(())
}