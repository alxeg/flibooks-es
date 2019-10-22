use elastic::prelude::*;
use futures::Future;
use serde_json;
use std::error::Error;
use std::fs;
use std::io::BufRead;
use std::io::BufReader;
use tokio_core::reactor::Core;
use uuid::Uuid;
use zip;

use conf;

pub fn start(file_name: &str) -> Result<(), Box<dyn Error>> {
    let mut core = Core::new()?;

    let settings = conf::SETTINGS.read()?;
    let base_url = (&settings.elastic_url).as_str();
    let index = (&settings.elastic_index).as_str();

    info!("Using the elasticsearch at '{}'", base_url);

    let client = AsyncClientBuilder::new()
        .base_url(base_url)
        .build(&core.handle())?;

    info!("Parsing the '{}' file", file_name);
    let file = fs::File::open(file_name)?;

    let mut archive = zip::ZipArchive::new(file)?;

    for i in 0..archive.len() {
        let file = archive.by_index(i)?;

        if file.name().ends_with(".inp") {
            let mut bulk = String::new();

            let inpx = format!("{}", file.name());
            let container = inpx.replace(".inp", ".zip");

            let breader = BufReader::new(file);
            for line in breader.lines() {
                let l = line?;
                let mut rec = process_book(l.split("\x04").collect());
                rec["container"] = json!(container);

                let header = serde_json::to_string(&json!({
                    "index": {
                        "_index": index,
                        "_type" : "book",
                        "_id": Uuid::new_v4(),
                    }}))?.to_string();

                bulk.push_str(&header);
                bulk.push_str("\n");
                bulk.push_str(&serde_json::to_string(&rec)?.to_string());
                bulk.push_str("\n");
            }

            let res_future = client
                .request(BulkRequest::new(bulk))
                .send()
                .and_then(|res| res.into_response::<BulkResponse>());

            let bulk_future = res_future.and_then(|bulk| {
                for op in bulk {
                    match op {
                        Err(op) => error!("error processing document: {:?}", op),
                        _ => (),
                    }
                }
                Ok(())
            });

            core.run(bulk_future)?;
        }
    }
    Ok(())
}

fn process_book(fields: Vec<&str>) -> serde_json::Value {
    let authors: Vec<_> = fields[0].split(":").filter(|s| !s.is_empty()).collect();
    let genres: Vec<_> = fields[1].split(":").filter(|s| !s.is_empty()).collect();

    json!({
        "title":        fields[2],
        "authors":      authors,
        "genres":       genres,
        "series":       fields[3],
        "ser_no":       fields[4].parse::<i32>().unwrap_or(0),
        "file":         fields[5],
        "file_size":    fields[6].parse::<i32>().unwrap_or(0),
        "lib_id":       fields[7],
        "del":          fields[8],
        "ext":          fields[9],
        "date":         fields[10],
        "lang":         fields[11],
    })
}
