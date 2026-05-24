use config::{Config, ConfigError, Environment, File};
use serde::Deserialize;
use std::env;
use std::sync::RwLock;

#[derive(Debug, Deserialize)]
pub struct Settings {
    pub log_level: String,
    pub log_config: String,
    pub elastic_url: String,
    pub elastic_login: String,
    pub elastic_password: String,
    pub elastic_skip_tls_verify: bool,
    pub elastic_index: String,
    pub listen_address: String,
    pub fb2c_path: String,
    pub data_dir: String,
    pub static_dir: String,
    pub static_route: String,
}

lazy_static::lazy_static! {
    pub static ref SETTINGS: RwLock<Settings> = RwLock::new(Settings::new().unwrap());
}

impl Settings {
    pub fn new() -> Result<Self, ConfigError> {
        let s = Config::builder()
            .set_default("log_level", "info")?
            .set_default("log_config", "log4rs.yml")?
            .set_default("elastic_url", "http://localhost:9200")?
            .set_default("elastic_index", "flibooks")?
            .set_default("elastic_skip_tls_verify", false)?
            .set_default("elastic_login", "admin")?
            .set_default("listen_address", "localhost:3000")?
            .set_default("fb2c_path", "./fb2c")?
            .set_default("data_dir", "")?
            .set_default("static_dir", "./static")?
            .set_default("static_route", "/")?
            .add_source(File::with_name("flibooks").required(false))
            .add_source(
                File::with_name(env::var("FLI_CONFIG").unwrap_or_default().as_str())
                    .required(false),
            )
            .add_source(Environment::with_prefix("fli"))
            .build()?;

        s.try_deserialize()
    }
}
