use log::debug;
use std::error::Error;
use std::path::Path;
use std::process::Command;

use crate::conf;

lazy_static::lazy_static! {
    pub static ref FB2C_CONVERTER: Fb2cConverter = Fb2cConverter::new().unwrap();
}

pub struct Fb2cConverter {
    path: String,
}

impl Fb2cConverter {
    fn new() -> Result<Self, String> {
        let settings = conf::SETTINGS.read();
        let path = match settings {
            Ok(s) => s.fb2c_path.clone(),
            Err(_) => return Err("Failed to read settings".to_string()),
        };

        let full_path = if Path::new(&path).is_absolute() {
            // Absolute path - validate it exists
            if !Path::new(&path).exists() {
                return Err(format!("fb2c not found at: {}", path));
            }
            path
        } else {
            // Relative path - try to find it in PATH
            let cmd_path = which::which(&path).map_err(|_| {
                format!("fb2c not found at '{}' and not in PATH", path)
            })?;
            cmd_path.to_string_lossy().into_owned()
        };

        Ok(Fb2cConverter { path: full_path })
    }

    pub fn convert(&self, src: &str, dst_dir: &str, format: &str) -> Result<(), Box<dyn Error>> {
        let cmd = format!(
            "{} convert --nodirs --ow --to {} {} {}",
            self.path, format, src, dst_dir
        );
        debug!("Executing conversion command: {}", cmd);

        let output = Command::new(&self.path)
            .arg("convert")
            .arg("--nodirs")
            .arg("--ow")
            .arg("--to")
            .arg(format)
            .arg(src)
            .arg(dst_dir)
            .output()?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!("fb2c conversion failed: {}", stderr).into())
        }
    }
}

// get_allowed_formats returns list of supported output formats
#[allow(dead_code)]
fn get_allowed_formats() -> Vec<&'static str> {
    vec!["epub", "azw3", "mobi"]
}

pub fn get_format_content_type(format: &str) -> Option<&'static str> {
    match format {
        "epub" => Some("application/epub+zip"),
        "azw3" => Some("application/vnd.amazon.ebook"),
        "mobi" => Some("application/vnd.amazon.mobi8-ebook"),
        _ => None,
    }
}
