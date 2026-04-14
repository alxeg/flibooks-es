use log::{info, error};
use reqwest::header::CONTENT_TYPE;
use serde_json::{json, Value};
use std::error::Error;
use std::fs::File;
use std::io::{BufRead, BufReader};
use uuid::Uuid;
use zip::ZipArchive;

use crate::conf;

pub async fn start(file_name: &str) -> Result<(), Box<dyn Error>> {
    let settings = conf::SETTINGS.read()?;

    let url = settings.elastic_url.clone();
    let login = settings.elastic_login.clone();
    let password = settings.elastic_password.clone();
    let index = settings.elastic_index.clone();

    info!("Using the elasticsearch at '{}'", url);
    info!("Parsing the '{}' file", file_name);

    // Create auth header
    let auth_value = format!(
        "Basic {}",
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, format!("{}:{}", login, password))
    );

    // Create client with headers
    let client = reqwest::Client::new();

    let file = File::open(file_name)?;
    let mut archive = ZipArchive::new(file)?;

    for i in 0..archive.len() {
        let file = archive.by_index(i)?;

        if file.name().ends_with(".inp") {
            let mut bulk = String::new();

            let inpx = format!("{}", file.name());
            let container = inpx.replace(".inp", ".zip");

            let breader = BufReader::new(file);
            for line in breader.lines() {
                let l = line?;
                let mut rec = process_book(l.split('\x04').collect());
                rec["container"] = json!(container);

                let header = json!({
                    "index": {
                        "_index": index,
                        "_id": Uuid::new_v4().to_string(),
                    }
                });

                bulk.push_str(&serde_json::to_string(&header)?);
                bulk.push_str("\n");
                bulk.push_str(&serde_json::to_string(&rec)?);
                bulk.push_str("\n");
            }

            let bulk_url = format!("{}/_bulk", url);
            let response = client
                .post(&bulk_url)
                .bearer_auth(&password)
                .header("Authorization", &auth_value)
                .header(CONTENT_TYPE, "application/x-ndjson")
                .body(bulk)
                .send()
                .await?;

            if response.status().is_success() {
                let body: Value = response.json().await?;
                if let Some(errors) = body.get("errors").and_then(|v| v.as_bool()) {
                    if errors {
                        error!("Bulk indexing had errors");
                    }
                }
                info!("Successfully indexed bulk data");
            } else {
                let status = response.status();
                let body = response.text().await?;
                error!("Error processing bulk: {} - {}", status, body);
            }
        }
    }
    Ok(())
}

fn process_book(fields: Vec<&str>) -> Value {
    let authors: Vec<_> = fields[0]
        .split(':')
        .filter(|s| !s.is_empty())
        .collect();
    let genres: Vec<_> = fields[1]
        .split(':')
        .filter(|s| !s.is_empty())
        .collect();

    json!({
        "title": fields[2],
        "authors": authors,
        "genres": genres,
        "series": fields[3],
        "ser_no": fields[4].parse::<i32>().unwrap_or(0),
        "file": fields[5],
        "file_size": fields[6].parse::<i32>().unwrap_or(0),
        "lib_id": fields[7],
        "del": fields[8],
        "ext": fields[9],
        "date": fields[10],
        "lang": fields[11],
    })
}
