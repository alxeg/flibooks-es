mod conf;
mod convert;
mod logger;
mod parse;
mod serve;

#[tokio::main]
async fn main() {
    logger::setup().unwrap();

    let matches = clap::Command::new("flibooks-es")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Flibusta's backups books search (via ES)")
        .subcommand(
            clap::Command::new("parse")
                .about("Parses the flibooks backup index file")
                .arg(
                    clap::Arg::new("inpx")
                        .short('i')
                        .long("inpx")
                        .value_name("INPX_FILE")
                        .help("Flibooks backup index file to be parsed")
                        .required(true),
                ),
        )
        .subcommand(clap::Command::new("serve").about("Serves the REST API (Default)"))
        .get_matches();

    match matches.subcommand_matches("parse") {
        Some(parse_args) => {
            let inpx_file = parse_args.get_one::<String>("inpx").unwrap();
            parse::start(inpx_file).await.unwrap();
        }
        _ => {
            serve::start().await.unwrap();
        }
    }
}
