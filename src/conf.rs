use config::{Config, ConfigError, Environment, File};
use std::env;
use std::sync::RwLock;

#[derive(Debug, Deserialize)]
pub struct Settings {
    pub log_level: String,
    pub log_config: String,
    pub elastic_url: String,
    pub elastic_index: String,
    pub listen_address: String,
}

lazy_static! {
    pub static ref SETTINGS: RwLock<Settings> = RwLock::new(Settings::new().unwrap());
}

impl Settings {
    pub fn new() -> Result<Self, ConfigError> {
        let s = Config::builder()
            .set_default("log_level", "info")?
            .set_default("log_config", "log4rs.yml")?
            .set_default("elastic_url", "http://localhost:9200")?
            .set_default("elastic_index", "flibooks")?
            .set_default("listen_address", "localhost:3000")?
            .add_source(File::with_name("flibooks").required(false))
            .add_source(File::with_name(env::var("FLI_CONFIG").unwrap_or_default().as_str()).required(false))
            .add_source(Environment::with_prefix("fli"))
            .build()?;

            s.try_deserialize()
    }
}
