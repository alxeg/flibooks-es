use log::{error, info, debug, warn};
use reqwest::header::CONTENT_TYPE;
use serde_json::{json, Value};
use std::error::Error;
use std::fs::File;
use std::io::{BufRead, BufReader};
use uuid::Uuid;
use zip::ZipArchive;

use crate::conf;

pub async fn start(file_name: &str) -> Result<(), Box<dyn Error>> {
    let url;
    let login;
    let password;
    let index;
    let data_dir;
    {
        let settings = conf::SETTINGS.read()?;
        url = settings.elastic_url.clone();
        login = settings.elastic_login.clone();
        password = settings.elastic_password.clone();
        index = settings.elastic_index.clone();
        data_dir = settings.data_dir.clone();
    }

    info!("Using the elasticsearch at '{}'", url);
    info!("Parsing the '{}' file", file_name);

    // Create client with headers
    let client = reqwest::Client::new();

    let file_path = if std::path::Path::new(&file_name).is_relative() && !data_dir.is_empty() {
        std::path::Path::new(&data_dir).join(file_name)
    } else {
        std::path::Path::new(file_name).to_path_buf()
    };

    let file = File::open(&file_path)?;
    let mut archive = ZipArchive::new(file)?;

    for i in 0..archive.len() {
        let file = archive.by_index(i)?;

        if file.name().ends_with(".inp") {
            let mut bulk = String::new();

            let inpx = file.name().to_string();
            debug!("Parsing the inp entry '{}'", inpx);

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
                bulk.push('\n');
                bulk.push_str(&serde_json::to_string(&rec)?);
                bulk.push('\n');
            }

            let bulk_url = format!("{}/{}/_bulk", url, index);
            debug!("Trying to insert parsed bulk data of {} to es url {}", inpx, bulk_url);

            let response = client
                .post(&bulk_url)
                .basic_auth(&login, Some(&password))
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
                info!("Successfully indexed bulk data for {}", inpx);
            } else {
                let status = response.status();
                let body = response.text().await?;
                error!("Error processing bulk {}: {} - {}", inpx, status, body);
            }
        } else {
            warn!("Skipping the inp entry {}", file.name());
        }
    }
    Ok(())
}

fn process_book(fields: Vec<&str>) -> Value {
    let authors: Vec<_> = fields[0].split(':').filter(|s| !s.is_empty()).collect();
    let genres: Vec<_> = fields[1].split(':').filter(|s| !s.is_empty()).collect();

    json!({
        "title": fields[2],
        "authors": authors,
        "genres": genres,
        "series": fields[3],
        "ser_no": fields[4],
        "file": fields[5],
        "file_size": fields[6].parse::<i32>().unwrap_or(0),
        "lib_id": fields[7],
        "del": fields[8],
        "ext": fields[9],
        "date": fields[10],
        "lang": fields[11],
    })
}
