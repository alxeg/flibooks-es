use bodyparser;
use elastic::prelude::*;
use iron::modifiers::Header;
use iron::prelude::*;
use iron::{headers, status};
use jsonpath::Selector;
use persistent;
use serde_json;
use serde_json::Value;
use std::error::Error;
use std::io::Read;
use std::iter::Iterator;

use conf;

pub mod request;

const MAX_BODY_LENGTH: usize = 1024 * 1024 * 10;

lazy_static! {
    pub static ref ES: SyncClient = connect_es().unwrap();
}

pub fn connect_es() -> Result<SyncClient, Box<Error>> {
    match conf::SETTINGS.read() {
        Ok(settings) => {
            let base_url = settings.elastic_base_url.as_str();

            info!("Using the elasticsearch at '{}'", base_url);

            Ok(SyncClientBuilder::new().base_url(base_url).build()?)
        }
        _ => Err(From::from("Failed to read settings")),
    }
}

pub fn start() -> Result<(), Box<Error>> {
    let settings = conf::SETTINGS.read()?;

    let listen = &settings.listen_address;

    let router = router!{
        authors: post "/api/author/search" => authors_handler,
    };

    let mut chain = Chain::new(router);
    chain.link_before(persistent::Read::<bodyparser::MaxBodyLength>::one(
        MAX_BODY_LENGTH,
    ));

    info!("Serving the API on {}", listen);
    Iron::new(chain).http(listen)?;

    Ok(())
}

fn authors_handler(req: &mut Request) -> IronResult<Response> {
    let body = req.get::<bodyparser::Struct<request::Author>>();
    let mut _error_response = Response::with((status::BadRequest, "Internal Server Error"));

    match body {
        Ok(Some(author_req)) => {
            debug!("Parsed author request:\n{:?}", author_req);

            match search_es(
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
                "$.aggregations.author.bucke1ts",
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

fn search_es(query: Value, filter: &str) -> Result<String, Box<Error>> {
    let mut result = String::new();

    let req = {
        let body = query;
        SearchRequest::new(body)
    };

    let mut raw = String::new();
    ES.request(req).send()?.into_raw().read_to_string(&mut raw)?;
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
