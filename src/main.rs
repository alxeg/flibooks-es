extern crate bodyparser;
extern crate config;
extern crate elastic;
extern crate futures;
extern crate iron;
extern crate jsonpath;
extern crate log4rs;
extern crate persistent;
extern crate tokio_core;
extern crate uuid;
extern crate zip;

#[macro_use]
extern crate router;
#[macro_use]
extern crate serde_json;
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate log;
#[macro_use]
extern crate clap;

pub mod conf;
pub mod logger;
pub mod parse;
pub mod serve;

use clap::{App, Arg, SubCommand};

fn main() {
    logger::setup().unwrap();

    let matches = App::new("flibooks-es")
        .version(crate_version!())
        .about("Flibusta's backups books search (via ES)")
        .subcommand(
            SubCommand::with_name("parse")
                .about("Parses the flibooks backup index file")
                .arg(
                    Arg::with_name("inpx")
                        .short("i")
                        .long("inpx")
                        .value_name("INPX_FILE")
                        .help("Flibooks backup index file to be parsed")
                        .required(true)
                        .takes_value(true),
                ),
        )
        .subcommand(SubCommand::with_name("serve").about("Serves the REST API (Default)"))
        .get_matches();

    match matches.subcommand_matches("parse") {
        Some(parse_args) => {
            let inpx_file = parse_args.value_of("inpx").unwrap();
            parse::start(inpx_file).unwrap();
        }
        _ => {
            serve::start().unwrap();
        }
    }
}
