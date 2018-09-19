use log::LevelFilter;
use log4rs;
use log4rs::append::console::ConsoleAppender;
use log4rs::config::{Appender, Config, Root};
use log4rs::encode::pattern::PatternEncoder;
use std::error::Error;
use std::path::Path;

use conf;

pub fn setup() -> Result<(), Box<Error>> {
    let settings = conf::SETTINGS.read()?;
    let log_config = &settings.log_config.as_str();

    if Path::new(log_config).exists() {
        log4rs::init_file(log_config, Default::default())?;
    } else {
        let log_level = match settings.log_level.to_lowercase().as_str() {
            "info" => LevelFilter::Info,
            "debug" => LevelFilter::Debug,
            "trace" => LevelFilter::Trace,
            "warn" => LevelFilter::Warn,
            "error" => LevelFilter::Error,
            _ => LevelFilter::Off,
        };

        let stdout = ConsoleAppender::builder()
            .encoder(Box::new(PatternEncoder::new(
                "{d(%+)(local)} {h({l})} [{t}] [{f}:{L}] {m}{n}",
            ))).build();
        let config = Config::builder()
            .appender(Appender::builder().build("stdout", Box::new(stdout)))
            .build(Root::builder().appender("stdout").build(log_level))
            .unwrap();

        log4rs::init_config(config)?;
        info!("No log4rs config file found. Using the default one");
    }

    Ok(())
}
