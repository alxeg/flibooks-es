use bodyparser;
use elastic::prelude::*;
use iron::modifiers::Header;
use iron::prelude::*;
use iron::{headers, status};
use jsonpath::Selector;
use persistent;
use router::Router;
use serde_json;
use serde_json::Value;
use std::error::Error;
use std::io::Read;
use std::iter::Iterator;

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
            let base_url = settings.elastic_base_url.as_str();

            info!("Using the elasticsearch at '{}'", base_url);

            Ok(SyncClientBuilder::new().base_url(base_url).build()?)
        }
        _ => Err(From::from("Failed to read settings")),
    }
}

fn es_search(query: Value, filter: &str) -> Result<String, Box<Error>> {
    let mut result = String::new();

    let req = {
        let body = query;
        SearchRequest::new(body)
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

fn compose_es_request(search: request::Search, s_type: SearchType) -> serde_json::Value {
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
            vec.push(json!({"wildcard": { "authors": search.author }}));
            vec.push(json!({"wildcard": { "title": search.title }}));
            vec
        }
        SearchType::SeriesSearch => {
            let mut vec = Vec::new();
            vec.push(json!({"wildcard": { "authors": search.author }}));
            vec.push(json!({"wildcard": { "series": search.series }}));
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

    debug!("es request {:?}", req);

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
                Header(headers::ContentType::json()),
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

            match es_search(
                json!({
                    "size": 0,
                    "aggs": {
                        "author": {
                        "terms": {
                            "field": "authors.keyword",
                            "include": author_req.author,
                            "size": author_req.limit
                            }
                        }
                }}),
                "$.aggregations.author.buckets",
            ) {
                Ok(search_result) => {
                    return Ok(Response::with((
                        status::Ok,
                        Header(headers::ContentType::json()),
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
            debug!("Author's books request:\n{:?}", search);
            let req = compose_es_request(search, SearchType::AuthorsBooks);

            match es_search(req, "$.hits.hits") {
                Ok(search_result) => {
                    return Ok(Response::with((
                        status::Ok,
                        Header(headers::ContentType::json()),
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
            debug!("Books title search:\n{:?}", search);
            let req = compose_es_request(search, SearchType::TitlesSearch);
            match es_search(req, "$.hits.hits") {
                Ok(search_result) => {
                    return Ok(Response::with((
                        status::Ok,
                        Header(headers::ContentType::json()),
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
            debug!("Books series search:\n{:?}", search);
            let req = compose_es_request(search, SearchType::SeriesSearch);
            match es_search(req, "$.hits.hits") {
                Ok(search_result) => {
                    return Ok(Response::with((
                        status::Ok,
                        Header(headers::ContentType::json()),
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

fn download_handler(req: &mut Request) -> IronResult<Response> {
    let mut _error_response = Response::with((status::BadRequest, "Server Error"));

    let ref id = req.extensions.get::<Router>().unwrap().find("id").unwrap();

    error!("Responding with error: {}", _error_response);
    Ok(_error_response)
}
