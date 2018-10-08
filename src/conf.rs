use config::{Config, ConfigError, Environment, File};
use std::env;
use std::sync::RwLock;

#[derive(Debug, Deserialize)]
pub struct Settings {
    pub log_level: String,
    pub log_config: String,
    pub elastic_base_url: String,
    pub listen_address: String,
}

lazy_static! {
    pub static ref SETTINGS: RwLock<Settings> = RwLock::new(Settings::new().unwrap());
}

impl Settings {
    pub fn new() -> Result<Self, ConfigError> {
        let mut s = Config::new();

        // set defaults
        s.set_default("log_level", "info")?;
        s.set_default("log_config", "log4rs.yml")?;
        s.set_default("elastic_base_url", "http://localhost:9200/flibooks")?;
        s.set_default("listen_address", "localhost:3000")?;

        s.merge(File::with_name("flibooks").required(false))?;

        match env::var("FLI_CONFIG") {
            Ok(config_file) => {
                s.merge(File::with_name(config_file.as_str()))?;
            }
            _ => (),
        }

        s.merge(Environment::with_prefix("fli"))?;

        // parse to struct
        s.try_into()
    }
}
